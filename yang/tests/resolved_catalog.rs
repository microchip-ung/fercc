// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::{Map, Value as Json};

use yang::resolved_catalog::to_resolved_catalog;
use yang::schema::Schema;

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test-data")
}

fn schema_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("schema")
        .join("resolved-yang-catalog-schema-v1.0.0.json")
}

fn validator() -> Validator {
    let text = fs::read_to_string(schema_path()).expect("read resolved-yang-catalog-schema");
    let schema: Json = serde_json::from_str(&text).expect("parse resolved-yang-catalog-schema");
    jsonschema::validator_for(&schema).expect("compile resolved-yang-catalog-schema")
}

fn read_bundled_catalog_files() -> (Vec<String>, Vec<String>) {
    let tarball = test_data_dir().join("b9a283a2c36b799683c792d8a0c21559.tar.gz");
    let file = fs::File::open(&tarball).expect("open bundled catalog tarball");
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let mut yang_sources = Vec::new();
    let mut sid_sources = Vec::new();
    for entry in archive.entries().expect("read tar entries") {
        let mut entry = entry.expect("tar entry");
        let path = entry.path().expect("entry path").to_path_buf();
        if path.parent().is_some_and(|p| p != Path::new("")) {
            continue;
        }
        let mut content = String::new();
        std::io::Read::read_to_string(&mut entry, &mut content).expect("read entry content");
        match path.extension().and_then(|e| e.to_str()) {
            Some("yang") => yang_sources.push(content),
            Some("sid") => sid_sources.push(content),
            _ => {}
        }
    }
    (yang_sources, sid_sources)
}

fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let (yang_sources, sid_sources) = read_bundled_catalog_files();
        yang::schema::build(&yang_sources, &sid_sources).expect("build schema from bundled catalog")
    })
}

const KNOWN_MISSING_SID_PATHS: &[&str] = &[
    "/ietf-interfaces:interfaces/interface/ietf-ip:ipv4/mchp-velocitysp-ip:statistics/clear/input",
    "/ietf-interfaces:interfaces/interface/ietf-ip:ipv4/mchp-velocitysp-ip:statistics/clear/output",
    "/ietf-interfaces:interfaces/interface/mchp-velocitysp-acl:acl-port/clear/input",
    "/ietf-interfaces:interfaces/interface/mchp-velocitysp-acl:acl-port/clear/output",
];

fn doc() -> Json {
    to_resolved_catalog(schema(), "b9a283a2c36b799683c792d8a0c21559")
        .expect("serialize resolved yang catalog")
}

#[test]
fn the_bundled_catalog_tolerates_two_known_synthetic_input_output_sid_gaps() {
    // mchp-velocitysp-ip's and mchp-velocitysp-acl's "clear" actions
    // declare no explicit input/output substatement, and whatever
    // generated this catalog's .sid files never assigned the resulting
    // synthetic input/output nodes a SID (RFC 9595 6.4 says they always
    // MUST get one) -- a known, temporary pyang bug (see notes.txt),
    // not a fercc bug. to_resolved_catalog tolerates exactly this one
    // case (synthetic input/output, no sid): the dump still succeeds,
    // and these four nodes alone come out with no "sid" field.
    let d = doc();
    for path in KNOWN_MISSING_SID_PATHS {
        let node = &d["schema-nodes"][*path];
        assert!(
            node.get("sid").is_none(),
            "{path} unexpectedly has a sid now -- the known pyang gap may be fixed; \
             update this test and the TODO in resolved_catalog.rs::schema_node_entry"
        );
    }
}

#[test]
fn every_cross_reference_in_the_document_resolves() {
    // JSON Schema can check shape and type but not that a string value
    // is actually a key somewhere else in the same document -- this
    // walks every such reference by hand against the real catalog.
    let d = doc();
    let mut errors = Vec::new();

    let schema_nodes = d["schema-nodes"].as_object().unwrap();
    let yang_data = d["yang-data"].as_object().unwrap();
    let typedefs = d["typedefs"].as_object().unwrap();
    let identities = d["identities"].as_object().unwrap();
    let modules = d["modules"].as_object().unwrap();
    let sid_index = d["sid-index"].as_object().unwrap();

    let check_node_path = |path: &str, map: &Map<String, Json>, map_name: &str, from: &str| {
        if !map.contains_key(path) {
            return Some(format!("{from}: {path:?} not found in {map_name}"));
        }
        None
    };

    for (own_map, map_name) in [(schema_nodes, "schema-nodes"), (yang_data, "yang-data")] {
        for (path, node) in own_map {
            let from = format!("{map_name}[{path:?}]");
            if let Some(parent) = node["parent"].as_str() {
                if let Some(e) =
                    check_node_path(parent, own_map, map_name, &format!("{from}.parent"))
                {
                    errors.push(e);
                }
            }
            for field in ["children", "data-children", "keys"] {
                if let Some(items) = node.get(field).and_then(Json::as_array) {
                    for item in items {
                        let p = item.as_str().unwrap();
                        if let Some(e) =
                            check_node_path(p, own_map, map_name, &format!("{from}.{field}"))
                        {
                            errors.push(e);
                        }
                    }
                }
            }
            if let Some(t) = node.get("type") {
                check_type_descriptor(
                    t,
                    typedefs,
                    &schema_nodes_and_yang_data(schema_nodes, yang_data),
                    &from,
                    &mut errors,
                );
            }
        }
    }

    for (key, t) in typedefs {
        let from = format!("typedefs[{key:?}]");
        check_type_descriptor(
            &t["type"],
            typedefs,
            &schema_nodes_and_yang_data(schema_nodes, yang_data),
            &from,
            &mut errors,
        );
    }

    for (key, identity) in identities {
        let from = format!("identities[{key:?}]");
        for field in ["base", "derived"] {
            for item in identity[field].as_array().unwrap() {
                let name = item.as_str().unwrap();
                if !identities.contains_key(name) {
                    errors.push(format!("{from}.{field}: {name:?} not found in identities"));
                }
            }
        }
    }

    for (mod_name, module) in modules {
        let from = format!("modules[{mod_name:?}]");
        for field in ["root-nodes", "data-root-nodes"] {
            for item in module[field].as_array().unwrap() {
                let p = item.as_str().unwrap();
                if !schema_nodes.contains_key(p) {
                    errors.push(format!("{from}.{field}: {p:?} not found in schema-nodes"));
                }
            }
        }
        for item in module["typedefs"].as_array().unwrap() {
            let key = item.as_str().unwrap();
            if !typedefs.contains_key(key) {
                errors.push(format!("{from}.typedefs: {key:?} not found in typedefs"));
            }
        }
        for item in module["identities"].as_array().unwrap() {
            let key = item.as_str().unwrap();
            if !identities.contains_key(key) {
                errors.push(format!(
                    "{from}.identities: {key:?} not found in identities"
                ));
            }
        }
    }

    for (sid, entry) in sid_index {
        let namespace = entry["namespace"].as_str().unwrap();
        let identifier = entry["identifier"].as_str().unwrap();
        let found = match namespace {
            "module" => modules.contains_key(identifier),
            "identity" => identities.contains_key(identifier),
            "data" => schema_nodes.contains_key(identifier) || yang_data.contains_key(identifier),
            other => {
                errors.push(format!("sid-index[{sid:?}]: unknown namespace {other:?}"));
                continue;
            }
        };
        if !found {
            errors.push(format!(
                "sid-index[{sid:?}]: {namespace}:{identifier:?} not found"
            ));
        }
    }

    // And the reverse direction: every sid-bearing entity must appear
    // in sid-index under its own sid.
    for (mod_name, module) in modules {
        let sid = module["sid"].as_str().unwrap();
        if sid_index.get(sid).map(|e| e["identifier"].as_str()) != Some(Some(mod_name.as_str())) {
            errors.push(format!(
                "module {mod_name:?}'s sid {sid:?} not reverse-indexed"
            ));
        }
    }
    for (key, identity) in identities {
        let sid = identity["sid"].as_str().unwrap();
        if sid_index.get(sid).map(|e| e["identifier"].as_str()) != Some(Some(key.as_str())) {
            errors.push(format!(
                "identity {key:?}'s sid {sid:?} not reverse-indexed"
            ));
        }
    }
    for (map, map_name) in [(schema_nodes, "schema-nodes"), (yang_data, "yang-data")] {
        for (path, node) in map {
            let Some(sid) = node.get("sid").and_then(Json::as_str) else {
                continue;
            };
            if sid_index.get(sid).map(|e| e["identifier"].as_str()) != Some(Some(path.as_str())) {
                errors.push(format!(
                    "{map_name}[{path:?}]'s sid {sid:?} not reverse-indexed"
                ));
            }
        }
    }

    assert!(
        errors.is_empty(),
        "{} referential-integrity error(s):\n{}",
        errors.len(),
        errors.join("\n")
    );
}

/// Collects every `derived-from` reference inside a single type-
/// descriptor JSON value (recursing into `union`'s member `type` array),
/// into `out`. Pure document traversal, no Schema involved.
fn collect_derived_from(ty: &Json, out: &mut std::collections::HashSet<String>) {
    if let Some(key) = ty.get("derived-from").and_then(Json::as_str) {
        out.insert(key.to_string());
    }
    if let Some(members) = ty.get("type").and_then(Json::as_array) {
        for member in members {
            collect_derived_from(member, out);
        }
    }
}

/// Collects every identityref `base` reference inside a single type-
/// descriptor JSON value (recursing into unions). Pure document
/// traversal, no Schema involved.
fn collect_identityref_bases(ty: &Json, out: &mut std::collections::HashSet<String>) {
    if ty.get("builtin").and_then(Json::as_str) == Some("identityref") {
        if let Some(bases) = ty.get("base").and_then(Json::as_array) {
            for b in bases {
                out.insert(b.as_str().unwrap().to_string());
            }
        }
    }
    if let Some(members) = ty.get("type").and_then(Json::as_array) {
        for member in members {
            collect_identityref_bases(member, out);
        }
    }
}

#[test]
fn the_document_has_no_orphan_schema_nodes_or_yang_data() {
    // Complements every_cross_reference_in_the_document_resolves's
    // forward direction (nothing is referenced that's absent) with the
    // reverse: nothing is present that isn't reachable by walking
    // "children" from a root (parent: null) -- purely from the
    // document's own fields, no Schema call involved. Together the two
    // tests prove the node set is exactly the tree implied by the
    // document's own parent/children links, not more and not fewer.
    let d = doc();
    for (map_name, map_key) in [("schema-nodes", "schema-nodes"), ("yang-data", "yang-data")] {
        let map = d[map_key].as_object().unwrap();
        let mut visited = std::collections::HashSet::new();
        let mut stack: Vec<&str> = map
            .iter()
            .filter(|(_, node)| node["parent"].is_null())
            .map(|(path, _)| path.as_str())
            .collect();
        while let Some(path) = stack.pop() {
            if visited.insert(path.to_string()) {
                if let Some(children) = map[path].get("children").and_then(Json::as_array) {
                    for child in children {
                        stack.push(child.as_str().unwrap());
                    }
                }
            }
        }
        let all_keys: std::collections::HashSet<String> = map.keys().cloned().collect();
        assert_eq!(
            visited, all_keys,
            "{map_name} has entries unreachable by walking children from a root"
        );
    }
}

#[test]
fn the_document_has_no_orphan_typedefs_or_identities() {
    // Complements every_cross_reference_in_the_document_resolves's
    // forward direction with the reverse: every typedefs/identities
    // entry must actually be pointed at from somewhere else in the
    // document (a type-descriptor's derived-from/base, or an identity's
    // own derived chain) -- entirely from the document's own fields, no
    // Schema call involved.
    let d = doc();
    let schema_nodes = d["schema-nodes"].as_object().unwrap().values();
    let yang_data = d["yang-data"].as_object().unwrap().values();
    let typedefs = d["typedefs"].as_object().unwrap();
    let identities = d["identities"].as_object().unwrap();

    let mut derived_from_refs = std::collections::HashSet::new();
    let mut identityref_base_refs = std::collections::HashSet::new();
    for node in schema_nodes.chain(yang_data) {
        if let Some(ty) = node.get("type") {
            collect_derived_from(ty, &mut derived_from_refs);
            collect_identityref_bases(ty, &mut identityref_base_refs);
        }
    }
    for t in typedefs.values() {
        collect_derived_from(&t["type"], &mut derived_from_refs);
    }

    for key in typedefs.keys() {
        assert!(
            derived_from_refs.contains(key),
            "typedefs[{key:?}] is never named by any derived-from in the document"
        );
    }

    // An identity is legitimately present either because some
    // identityref names it directly as a base, or because it's in the
    // derived-chain of such an identity (RFC 7950 9.10.2's legal value
    // space) -- expand the direct-base seeds via each identity's own
    // "derived" field, which is itself part of the document.
    let mut reachable = std::collections::HashSet::new();
    let mut stack: Vec<String> = identityref_base_refs.into_iter().collect();
    while let Some(key) = stack.pop() {
        if reachable.insert(key.clone()) {
            if let Some(identity) = identities.get(&key) {
                for d in identity["derived"].as_array().unwrap() {
                    stack.push(d.as_str().unwrap().to_string());
                }
            }
        }
    }
    for key in identities.keys() {
        assert!(
            reachable.contains(key),
            "identities[{key:?}] is never named as a base (directly or via derivation) by any identityref in the document"
        );
    }
}

#[test]
fn sid_index_count_matches_sid_bearing_entries_in_the_document() {
    // every_cross_reference_in_the_document_resolves already proves
    // every sid-bearing entity is reverse-indexed under its own sid, and
    // every sid-index entry resolves to a real entity -- but neither
    // direction catches a surplus sid-index entry pointing at an entity
    // that's already correctly indexed under a different sid. Counting
    // sid-bearing entries directly from the document (not from Schema)
    // and comparing against sid-index's own length catches that too.
    let d = doc();
    let mut sid_bearing = 0;
    for map_key in ["schema-nodes", "yang-data"] {
        sid_bearing += d[map_key]
            .as_object()
            .unwrap()
            .values()
            .filter(|n| n.get("sid").is_some())
            .count();
    }
    sid_bearing += d["identities"].as_object().unwrap().len();
    sid_bearing += d["modules"]
        .as_object()
        .unwrap()
        .values()
        .filter(|m| m.get("sid").is_some())
        .count();
    assert_eq!(d["sid-index"].as_object().unwrap().len(), sid_bearing);
}

fn schema_nodes_and_yang_data<'a>(
    schema_nodes: &'a Map<String, Json>,
    yang_data: &'a Map<String, Json>,
) -> Vec<&'a Map<String, Json>> {
    vec![schema_nodes, yang_data]
}

fn check_type_descriptor(
    ty: &Json,
    typedefs: &Map<String, Json>,
    node_maps: &[&Map<String, Json>],
    from: &str,
    errors: &mut Vec<String>,
) {
    if let Some(key) = ty.get("derived-from").and_then(Json::as_str) {
        if !typedefs.contains_key(key) {
            errors.push(format!(
                "{from}.type.derived-from: {key:?} not found in typedefs"
            ));
        }
    }
    if let Some(target) = ty.get("target").and_then(Json::as_str) {
        if !node_maps.iter().any(|m| m.contains_key(target)) {
            errors.push(format!(
                "{from}.type.target: {target:?} not found in schema-nodes or yang-data"
            ));
        }
    }
    if let Some(members) = ty.get("type").and_then(Json::as_array) {
        for member in members {
            check_type_descriptor(member, typedefs, node_maps, from, errors);
        }
    }
}

#[test]
fn resolved_catalog_validates_against_the_real_schema() {
    // Allow-listed by exact instance path: the four known, documented
    // synthetic-input/output sid gaps (see notes.txt and
    // the_bundled_catalog_tolerates_two_known_synthetic_input_output_sid_gaps
    // above) are the only schema violations this document is allowed to
    // have. Every other validation error is a real bug.
    let allowed_instance_paths: Vec<String> = KNOWN_MISSING_SID_PATHS
        .iter()
        .map(|p| format!("/schema-nodes/{}", p.replace('/', "~1")))
        .collect();

    let v = validator();
    let d = doc();
    let errors: Vec<_> = v.iter_errors(&d).collect();
    let unexpected: Vec<String> = errors
        .iter()
        .filter(|e| !allowed_instance_paths.contains(&e.instance_path().to_string()))
        .map(|e| e.to_string())
        .collect();
    assert!(
        unexpected.is_empty(),
        "resolved catalog failed schema validation:\n{}",
        unexpected.join("\n")
    );

    for path in &allowed_instance_paths {
        assert!(
            errors
                .iter()
                .any(|e| e.instance_path().to_string() == *path),
            "expected a known missing-sid gap at {path}, but it's gone now -- \
             the pyang bug may be fixed; remove it from KNOWN_MISSING_SID_PATHS \
             and the TODO in resolved_catalog.rs::schema_node_entry"
        );
    }
}

#[test]
fn typedef_entry_nests_its_type_descriptor_under_type() {
    // A typedef is not itself a type (RFC 7950 7.3): it names one via
    // its own "type" substatement. "interface-ref" is also a real
    // regression case for typedef-level leafref resolution -- its own
    // canonical descriptor's target must be resolved even though
    // nothing walks typedefs the way reachable schema nodes are walked.
    let d = doc();
    let t = &d["typedefs"]["ietf-interfaces interface-ref"];
    assert_eq!(t["module"], "ietf-interfaces");
    assert_eq!(t["name"], "interface-ref");
    assert!(
        t.get("builtin").is_none(),
        "type-descriptor fields must not be inlined onto the typedef itself"
    );
    assert_eq!(t["type"]["builtin"], "leafref");
    assert_eq!(
        t["type"]["target"],
        "/ietf-interfaces:interfaces/interface/name"
    );
}

#[test]
fn an_unreachable_typedef_is_excluded_from_the_document() {
    // "interface-state-ref" is a real typedef in this catalog's own
    // ietf-interfaces.yang, but its only use site (ietf-routing's
    // routing-state container) is deviated away by this device profile
    // -- nothing in the resolved tree actually uses it, so it must not
    // appear in `typedefs` or in ietf-interfaces's own `typedefs` list,
    // matching the schema's own "every named type used in this catalog"
    // description for that collection.
    let d = doc();
    assert!(d["typedefs"]
        .get("ietf-interfaces interface-state-ref")
        .is_none());
    let module_typedefs = d["modules"]["ietf-interfaces"]["typedefs"]
        .as_array()
        .unwrap();
    assert!(!module_typedefs
        .iter()
        .any(|v| v == "ietf-interfaces interface-state-ref"));
}

#[test]
fn module_entry_carries_real_sid_and_namespace() {
    let m = &doc()["modules"]["ietf-ip"];
    assert_eq!(m["namespace"], "urn:ietf:params:xml:ns:yang:ietf-ip");
    assert!(m["sid"].is_string());
}

#[test]
fn identity_entries_carry_real_base_and_derived() {
    let d = doc();
    let ethernet = &d["identities"]["iana-if-type:ethernetCsmacd"];
    assert!(ethernet["sid"].is_string());
    assert_eq!(
        ethernet["base"],
        serde_json::json!(["iana-if-type:iana-interface-type"])
    );
}

#[test]
fn a_real_leaf_node_carries_a_fully_resolved_type_and_sid() {
    let d = doc();
    let node = &d["schema-nodes"]["/ietf-interfaces:interfaces/interface/name"];
    assert_eq!(node["keyword"], "leaf");
    assert!(node["sid"].is_string());
    assert_eq!(node["type"]["builtin"], "string");
}

#[test]
fn a_real_list_node_carries_resolved_keys() {
    let d = doc();
    let node = &d["schema-nodes"]["/ietf-interfaces:interfaces/interface"];
    assert_eq!(node["keyword"], "list");
    assert_eq!(
        node["keys"],
        serde_json::json!(["/ietf-interfaces:interfaces/interface/name"])
    );
}

#[test]
fn a_real_choice_node_may_lack_a_sid() {
    let d = doc();
    let choice =
        &d["schema-nodes"]["/ietf-interfaces:interfaces/interface/ietf-ip:ipv4/address/subnet"];
    assert_eq!(choice["keyword"], "choice");
    // No assertion on choice["sid"] either way: optional per schema,
    // present or absent depending on a pyang-version-dependent quirk,
    // not an RFC 9595 requirement (RFC 7950 3's own "data node"
    // definition excludes choice/case).
    assert!(choice.get("sid").is_none() || choice["sid"].is_string());
}

#[test]
fn two_modules_augmenting_the_same_parent_resolve_to_independent_nodes() {
    // Regression coverage (real-data instance of the synthetic unit
    // test in yang::schema): mchp-velocitysp-acl and -acl2 both augment
    // /if:interfaces/if:interface with their own, differently-shaped
    // "acl-port" container. Each must keep its own sid and children.
    let d = doc();
    let v1 =
        &d["schema-nodes"]["/ietf-interfaces:interfaces/interface/mchp-velocitysp-acl:acl-port"];
    let v2 =
        &d["schema-nodes"]["/ietf-interfaces:interfaces/interface/mchp-velocitysp-acl2:acl-port"];
    assert_eq!(v1["keyword"], "container");
    assert_eq!(v2["keyword"], "container");
    assert!(v1["sid"].is_string());
    assert!(v2["sid"].is_string());
    assert_ne!(v1["sid"], v2["sid"]);
    assert_ne!(v1["children"], v2["children"]);
}

#[test]
fn yang_data_root_is_present_and_separate_from_schema_nodes() {
    let d = doc();
    let error_root = &d["yang-data"]["/ietf-coreconf:error"];
    assert_eq!(error_root["keyword"], "container");
    assert!(error_root["sid"].is_string());
    assert!(d["schema-nodes"].get("/ietf-coreconf:error").is_none());
}

#[test]
fn sid_index_round_trips_a_real_module_entry() {
    let d = doc();
    let module_sid = d["modules"]["ietf-ip"]["sid"].as_str().unwrap().to_string();
    assert_eq!(
        d["sid-index"][&module_sid],
        serde_json::json!({"namespace": "module", "identifier": "ietf-ip"})
    );
}

#[test]
fn leaf_and_leaf_list_nodes_have_no_children_fields() {
    // leaf/leaf-list/anydata/anyxml have no child schema-node
    // substatements per RFC 7950's grammar, so unlike every other
    // keyword they must not carry `children`/`data-children` at all --
    // not even as empty arrays.
    let d = doc();
    let leaf = &d["schema-nodes"]["/ietf-interfaces:interfaces/interface/name"];
    assert_eq!(leaf["keyword"], "leaf");
    assert!(leaf.get("children").is_none());
    assert!(leaf.get("data-children").is_none());

    let leaf_list = &d["schema-nodes"]
        ["/ieee802-dot1cb-stream-identification:stream-identity/out-facing/input-port"];
    assert_eq!(leaf_list["keyword"], "leaf-list");
    assert!(leaf_list.get("children").is_none());
    assert!(leaf_list.get("data-children").is_none());
}
