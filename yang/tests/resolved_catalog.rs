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

const IMPLEMENTED_YANG_DATA_ROOTS: &[&str] = &["/ietf-coreconf:error"];

const IDENTITY_ACCEPT_LISTS: &[(&str, &[&str])] = &[
    (
        "ietf-interfaces:interface-type",
        &[
            "iana-if-type:ethernetCsmacd",
            "iana-if-type:ieee8023adLag",
            "iana-if-type:l3ipvlan",
        ],
    ),
    (
        "ieee802-dot1q-types:type-of-operation",
        &[
            "ieee802-dot1q-sched:set-gate-states",
            "ieee802-dot1q-sched:set-and-hold-mac",
            "ieee802-dot1q-sched:set-and-release-mac",
        ],
    ),
    (
        "ieee802-dot1q-bridge:type-of-port",
        &[
            "ieee802-dot1q-bridge:c-vlan-bridge-port",
            "ieee802-dot1q-bridge:provider-network-port",
            "ieee802-dot1q-bridge:d-bridge-port",
            "mchp-velocitysp-bridge:provider-custom-network-port",
        ],
    ),
    (
        "iana-hardware:hardware-class",
        &[
            "iana-hardware:chassis",
            "iana-hardware:module",
            "iana-hardware:sensor",
            "iana-hardware:port",
            "iana-hardware:container",
        ],
    ),
    (
        "ietf-datastores:datastore",
        &[
            "ietf-datastores:running",
            "ietf-datastores:startup",
            "ietf-datastores:operational",
        ],
    ),
];

fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let (yang_sources, sid_sources) = read_bundled_catalog_files();
        yang::schema::build(
            &yang_sources,
            &sid_sources,
            IMPLEMENTED_YANG_DATA_ROOTS,
            IDENTITY_ACCEPT_LISTS,
        )
        .expect("build schema from bundled catalog")
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
fn maps_and_arrays_without_source_order_semantics_are_sorted() {
    let d = doc();

    let assert_keys_sorted = |map: &Map<String, Json>, name: &str| {
        let keys: Vec<&String> = map.keys().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "{name}'s keys aren't alphabetically sorted");
    };
    let assert_keys_sorted_numerically = |map: &Map<String, Json>, name: &str| {
        let keys: Vec<i64> = map.keys().map(|k| k.parse().unwrap()).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "{name}'s keys aren't numerically sorted");
    };
    let assert_strs_sorted = |array: &[Json], name: &str| {
        let items: Vec<&str> = array.iter().map(|v| v.as_str().unwrap()).collect();
        let mut sorted = items.clone();
        sorted.sort();
        assert_eq!(items, sorted, "{name} isn't alphabetically sorted");
    };

    assert_keys_sorted(d["modules"].as_object().unwrap(), "modules");
    assert_keys_sorted(
        d["import-only-modules"].as_object().unwrap(),
        "import-only-modules",
    );
    assert_keys_sorted(d["typedefs"].as_object().unwrap(), "typedefs");
    assert_keys_sorted(d["identities"].as_object().unwrap(), "identities");
    assert_keys_sorted(d["schema-nodes"].as_object().unwrap(), "schema-nodes");
    assert_keys_sorted(d["yang-data"].as_object().unwrap(), "yang-data");
    assert_keys_sorted_numerically(d["sid-index"].as_object().unwrap(), "sid-index");

    // A module's own `typedefs`/`identities` arrays are NOT checked
    // here -- see
    // a_modules_own_typedefs_and_identities_follow_source_declaration_
    // order_not_alphabetical for why: both are true per-module source
    // declaration order (typedefs() now predeclares eagerly and
    // depth-first; identities are registered per-module, in source
    // order, in ensure_module), so they must NOT be alphabetically
    // sorted in general.
    for (name, identity) in d["identities"].as_object().unwrap() {
        assert_strs_sorted(
            identity["derived"].as_array().unwrap(),
            &format!("identities[{name:?}].derived"),
        );
    }
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
fn yang_data_keys_must_be_schema_node_paths_not_bare_qualified_names() {
    // patternProperties alone can't make the schema validator reject an
    // unrecognized key shape -- a key matching no pattern is simply
    // skipped, not flagged (there's no additionalProperties/
    // unevaluatedProperties closing yang-data, by deliberate forward-
    // compat policy). So the schema's own pattern must be checked by
    // hand, against a synthetic malformed entry (the real document's
    // own key shape is checked generically by verify_resolved_catalog,
    // which to_resolved_catalog always runs).
    let v = validator();
    let base = serde_json::json!({
        "$schema": "urn:fercc:resolved-yang-catalog-schema:1.0.0",
        "schema-version": "1.0.0",
        "yang-catalog": "deadbeef",
        "modules": {},
        "typedefs": {},
        "identities": {},
        "schema-nodes": {},
        "sid-index": {}
    });

    let malformed_entry = serde_json::json!({"this is not a schema-node": true});

    let mut with_real_key_shape = base;
    with_real_key_shape["yang-data"] =
        serde_json::json!({ "/ietf-coreconf:error": malformed_entry });
    assert!(
        v.iter_errors(&with_real_key_shape).next().is_some(),
        "a malformed entry under the real \"/module:name\" key shape must fail validation"
    );
}

#[test]
fn typedef_entry_nests_its_type_descriptor_under_type() {
    // A typedef is not itself a type (RFC 7950 7.3): it names one via
    // its own "type" substatement. "interface-ref" is also a real
    // regression case confirming a typedef's own canonical descriptor
    // never claims a resolved leafref target -- a typedef has no
    // location in the schema tree, so there's no single node to
    // resolve "target" against (RFC 7950 9.9.2: the leafref's context
    // node is "the leaf or leaf-list node in the data tree that
    // references the typedef", never the typedef itself). Only an
    // actual use site's own resolved type (schema-nodes[...]) ever
    // gets a target.
    let d = doc();
    let t = &d["typedefs"]["ietf-interfaces:interface-ref"];
    assert_eq!(t["module"], "ietf-interfaces");
    assert_eq!(t["name"], "interface-ref");
    assert!(
        t.get("builtin").is_none(),
        "type-descriptor fields must not be inlined onto the typedef itself"
    );
    assert_eq!(t["type"]["builtin"], "leafref");
    assert!(
        t["type"].get("target").is_none(),
        "a typedef's own type descriptor must never carry a resolved leafref target"
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
        .get("ietf-interfaces:interface-state-ref")
        .is_none());
    let module_typedefs = d["modules"]["ietf-interfaces"]["typedefs"]
        .as_array()
        .unwrap();
    assert!(!module_typedefs
        .iter()
        .any(|v| v == "ietf-interfaces:interface-state-ref"));
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
fn the_bundled_ietf_sid_files_structure_is_interpreted_with_its_own_sids() {
    // ietf-sid-file defines the .sid file format itself as
    // `sx:structure sid-file { uses sid-file-contents; }` (RFC 8791), and
    // its own .sid file assigns SIDs 29001..29017 to that structure's
    // nodes, addressed under the structure's name. Every one of those
    // must resolve to a node of the structure: the root, its five leafs,
    // and its three lists with all of their own children, each SID on
    // exactly the node the .sid file names. Which children exist and
    // which SIDs they carry is taken from this catalog's own
    // ietf-sid-file.yang and ietf-sid-file@2023-10-27.sid, read by hand.
    let schema = schema();
    let names: Vec<String> = schema.structure_roots().map(|r| r.name.clone()).collect();
    assert_eq!(names, vec!["ietf-sid-file:sid-file"]);
    let root = schema.structure_roots().next().unwrap();
    assert_eq!(root.kw, "container");
    assert_eq!(root.sid, Some(29001));

    type Child = (
        &'static str,
        i64,
        &'static str,
        &'static [(&'static str, i64)],
    );
    let expected: &[Child] = &[
        (
            "assignment-range",
            29002,
            "list",
            &[("entry-point", 29003), ("size", 29004)],
        ),
        (
            "dependency-revision",
            29005,
            "list",
            &[("module-name", 29006), ("module-revision", 29007)],
        ),
        ("description", 29008, "leaf", &[]),
        (
            "item",
            29009,
            "list",
            &[
                ("identifier", 29010),
                ("namespace", 29011),
                ("sid", 29012),
                ("status", 29013),
            ],
        ),
        ("module-name", 29014, "leaf", &[]),
        ("module-revision", 29015, "leaf", &[]),
        ("sid-file-status", 29016, "leaf", &[]),
        ("sid-file-version", 29017, "leaf", &[]),
    ];
    let mut children: Vec<String> = root
        .children()
        .map(|c| c.local_name().to_string())
        .collect();
    children.sort();
    let mut expected_children: Vec<&str> = expected.iter().map(|e| e.0).collect();
    expected_children.sort();
    assert_eq!(children, expected_children);

    for &(name, sid, kw, grandchildren) in expected {
        let child = root.find_child(name).unwrap();
        assert_eq!(child.kw, kw, "{name}");
        assert_eq!(child.sid, Some(sid), "{name}");
        // RFC 8791: the config-stmt is ignored inside a structure.
        assert!(!child.config, "{name} must not be config");
        let mut names: Vec<String> = child
            .children()
            .map(|c| c.local_name().to_string())
            .collect();
        names.sort();
        let mut expected_names: Vec<&str> = grandchildren.iter().map(|g| g.0).collect();
        expected_names.sort();
        assert_eq!(names, expected_names, "children of {name}");
        for &(grandchild, grandchild_sid) in grandchildren {
            assert_eq!(
                child.find_child(grandchild).unwrap().sid,
                Some(grandchild_sid),
                "{name}/{grandchild}"
            );
        }
    }

    // The sid-file-contents grouping's own typedef uses resolve through
    // the structure: entry-point is ietf-sid-file's own `sid` typedef.
    let entry_point = root
        .find_child("assignment-range")
        .unwrap()
        .find_child("entry-point")
        .unwrap();
    assert_eq!(entry_point.ty().unwrap().base().unwrap().name, "sid");

    // A structure is not part of any datastore: it isn't a schema node,
    // a yang-data root, a wire-addressable SID, or anything the resolved
    // catalog reports.
    assert!(schema
        .node_by_schema_path("/ietf-sid-file:sid-file")
        .is_none());
    assert!(schema
        .yang_data_root_by_name("ietf-sid-file:sid-file")
        .is_none());
    assert!((29001..=29017).all(|sid| schema.node_by_sid(sid).is_none()));
    let d = doc();
    assert!(d["schema-nodes"].get("/ietf-sid-file:sid-file").is_none());
    assert!(d["yang-data"].get("/ietf-sid-file:sid-file").is_none());
    assert!((29001..=29017).all(|sid| d["sid-index"].get(sid.to_string()).is_none()));
}

#[test]
fn every_yang_data_node_reports_config_false() {
    // RFC 8040 Sec. 8: "The config-stmt is ignored if present" inside a
    // yang-data extension's content -- it isn't real config/state
    // data, so config must read false everywhere in a yang-data
    // subtree, never the ordinary config:true default a schema node
    // gets when nothing says otherwise.
    let d = doc();
    for (path, node) in d["yang-data"].as_object().unwrap() {
        if let Some(config) = node.get("config") {
            assert_eq!(config, false, "yang-data[{path:?}].config must be false");
        }
    }
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
fn every_module_in_the_real_catalog_partitions_as_hand_classified() {
    // Hardcoded by hand, by reading every one of the 62 real modules in
    // this bundled catalog's own source text (not derived from any
    // fercc code, including Schema's own classification logic -- that
    // would just prove the generator agrees with itself): RFC 7950
    // 5.6.5 says a module implements iff it implements at least one of
    // its own data nodes/RPCs/actions/notifications/deviations --
    // including a node its own augment places into another module's
    // tree, since that node's owner is the augmenting module, not the
    // target -- or an identity actually derived (RFC 7950 7.18.2:
    // irreflexively, so never the base identity itself) from a live
    // identityref's own base, narrowed by IDENTITY_ACCEPT_LISTS where
    // an entry applies (e.g. ieee802-dot1q-psfp's own
    // "set-gate-and-ipv" is excluded by its own "type-of-operation"
    // entry, which is why ieee802-dot1q-psfp -- declaring nothing else
    // -- is import-only despite its sibling psfp-bridge staying
    // implemented via its own real node content). A module not
    // implemented is import-only-shaped, but only actually listed if
    // something in the live, implemented tree genuinely resolves
    // against one of its own reusable definitions -- a typedef, an
    // identity, a grouping actually spliced in by a surviving `uses`,
    // or an extension actually invoked by a surviving node -- never
    // merely because some implemented module's text happens to
    // `import` it. This is why "iana-crypt-hash" (its own "crypt-hash"
    // typedef's one real use site, ietf-system's own "password" leaf,
    // is itself deviated away by ietf-system-dev) and
    // "ietf-yang-structure-ext" (its "sx:structure" extension is only
    // ever invoked inside ietf-sid-file's own text, which nothing live
    // implements either) are listed nowhere at all, despite being
    // imported by real, implemented modules. "ietf-netconf-acm" is the
    // one non-obvious implemented case: its own container is fully
    // deviated away by "ietf-netconf-acm-dev" (deviate not-supported),
    // but a deviated module is always implemented -- a server can't
    // implement "ietf-netconf-acm-dev"'s deviations of a module it
    // doesn't implement, and ietf-constrained-yang-library can only
    // report a module's deviations on its own `module` entry, never on
    // an `import-only-module` one. "ietf-restconf" is listed because
    // this device is a CoreCONF server, not a RESTCONF one: it never
    // actually invokes
    // "ietf-restconf"'s own `rc:yang-data` extension (`uses
    // errors;`/`uses restconf;` builds real nodes inside
    // "ietf-restconf" itself, but nothing live elsewhere ever uses
    // them) -- except "ietf-coreconf"'s own `coreconf-error` body,
    // which DOES invoke `rc:yang-data`, and that body IS implemented
    // (on IMPLEMENTED_YANG_DATA_ROOTS), so "ietf-restconf" is listed on
    // that real usage, not merely for existing in some import graph. If
    // this catalog's own modules ever change, this list must be
    // re-derived by hand again, not regenerated from fercc's own
    // classification. Disjointness and node-array shape
    // are already covered, independent of this hand classification, by
    // modules_and_import_only_modules_are_disjoint_and_import_only_
    // entries_lack_node_arrays -- this test is purely about whether the
    // membership itself is correct.
    const EXPECTED_IMPORT_ONLY: &[&str] = &[
        "ieee802-dot1ab-types",
        "ieee802-dot1q-preemption",
        "ieee802-dot1q-psfp",
        "ieee802-dot1q-stream-filters-gates",
        "ieee802-dot1q-types",
        "ieee802-types",
        "ietf-inet-types",
        "ietf-restconf",
        "ietf-routing-types",
        "ietf-sid-file",
        "ietf-yang-types",
    ];
    const EXPECTED_IMPLEMENTED: &[&str] = &[
        "iana-hardware",
        "iana-if-type",
        "ieee1588-ptp",
        "ieee1588-ptp-dev",
        "ieee802-dot1ab-lldp",
        "ieee802-dot1ab-lldp-dev",
        "ieee802-dot1as-hd",
        "ieee802-dot1as-ptp",
        "ieee802-dot1cb-stream-identification",
        "ieee802-dot1cb-stream-identification-dev",
        "ieee802-dot1q-bridge",
        "ieee802-dot1q-bridge-dev",
        "ieee802-dot1q-preemption-bridge",
        "ieee802-dot1q-psfp-bridge",
        "ieee802-dot1q-sched",
        "ieee802-dot1q-sched-bridge",
        "ieee802-ethernet-interface",
        "ietf-constrained-yang-library",
        "ietf-constrained-yang-library-dev",
        "ietf-coreconf",
        "ietf-datastores",
        "ietf-hardware",
        "ietf-hardware-dev",
        "ietf-igmp-mld-snooping",
        "ietf-igmp-mld-snooping-dev",
        "ietf-interfaces",
        "ietf-interfaces-dev",
        "ietf-ip",
        "ietf-ipv4-unicast-routing",
        "ietf-netconf-acm",
        "ietf-netconf-acm-dev",
        "ietf-routing",
        "ietf-routing-dev",
        "ietf-system",
        "ietf-system-dev",
        "mchp-velocitysp-acl",
        "mchp-velocitysp-acl2",
        "mchp-velocitysp-aggr",
        "mchp-velocitysp-bridge",
        "mchp-velocitysp-dhcp",
        "mchp-velocitysp-frame",
        "mchp-velocitysp-ip",
        "mchp-velocitysp-lldp",
        "mchp-velocitysp-port",
        "mchp-velocitysp-ptp",
        "mchp-velocitysp-redbox",
        "mchp-velocitysp-serdes",
        "mchp-velocitysp-system",
    ];

    let d = doc();
    let actual_modules: std::collections::HashSet<&str> = d["modules"]
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    let actual_import_only: std::collections::HashSet<&str> = d["import-only-modules"]
        .as_object()
        .unwrap()
        .values()
        .map(|m| m["name"].as_str().unwrap())
        .collect();

    let expected_modules: std::collections::HashSet<&str> =
        EXPECTED_IMPLEMENTED.iter().copied().collect();
    let expected_import_only: std::collections::HashSet<&str> =
        EXPECTED_IMPORT_ONLY.iter().copied().collect();

    assert_eq!(actual_modules, expected_modules);
    assert_eq!(actual_import_only, expected_import_only);
}

#[test]
fn a_modules_own_typedefs_and_identities_follow_source_declaration_order_not_alphabetical() {
    // Both arrays are real per-module source declaration order (see
    // yang/src/schema.rs's predeclare_typedefs and ensure_module's
    // per-module identity registration), not alphabetical -- confirmed
    // against this catalog's own ieee802-dot1q-bridge.yang (identities)
    // and ietf-hardware.yang (typedefs), hand-read directly from their
    // own source text, in the exact order their own `identity`/
    // `typedef` statements appear. Neither list is alphabetically
    // sorted, which is itself part of what this test proves -- a
    // regression back to alphabetical or first-use order would still
    // pass a bare "is this sorted" check but fail this one.
    //
    // ieee802-dot1q-bridge's own identities only lists 4 of its 7 real
    // "type-of-port"-derived identities here: "customer-network-port"/
    // "customer-edge-port"/"remote-customer-access-port" are pruned by
    // IDENTITY_ACCEPT_LISTS's own "ieee802-dot1q-bridge:type-of-port"
    // entry (the real device deviates its "port-type" leaf's own type
    // to a narrower identityref, "mchp-vlan-port-type", that excludes
    // them -- see mchp-velocitysp-bridge.yang's own doc comment on that
    // typedef and ietf-interfaces-dev.yang's deviation), so they're
    // unreachable identities entirely, not merely out of declaration
    // order.
    let d = doc();
    assert_eq!(
        d["modules"]["ieee802-dot1q-bridge"]["identities"],
        serde_json::json!([
            "ieee802-dot1q-bridge:type-of-port",
            "ieee802-dot1q-bridge:c-vlan-bridge-port",
            "ieee802-dot1q-bridge:provider-network-port",
            "ieee802-dot1q-bridge:d-bridge-port",
        ])
    );
    assert_eq!(
        d["modules"]["ietf-hardware"]["typedefs"],
        serde_json::json!([
            "ietf-hardware:sensor-value-type",
            "ietf-hardware:sensor-value-scale",
            "ietf-hardware:sensor-value-precision",
            "ietf-hardware:sensor-value",
            "ietf-hardware:sensor-status",
        ])
    );
}
