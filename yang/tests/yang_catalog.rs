// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::{Map, Value as Json};

use yang::schema::Schema;
use yang::yang_catalog::to_yang_catalog;

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test-data")
}

fn schema_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("schema")
        .join("yang-catalog-schema-v1.0.0.json")
}

fn validator() -> Validator {
    let text = fs::read_to_string(schema_path()).expect("read yang-catalog-schema");
    let schema: Json = serde_json::from_str(&text).expect("parse yang-catalog-schema");
    jsonschema::validator_for(&schema).expect("compile yang-catalog-schema")
}

fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let tarball = test_data_dir().join("9a836fc0bdf2b7b77ba49ea26c94261e.tar.gz");
        let bytes = fs::read(&tarball).expect("read bundled catalog tarball");
        yang::catalog::load_from_tarball(&bytes)
            .expect("load bundled catalog tarball")
            .build_schema()
            .expect("build schema from bundled catalog")
    })
}

fn doc() -> Json {
    to_yang_catalog(schema()).expect("serialize yang catalog")
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
}

#[test]
fn yang_catalog_validates_against_the_real_schema() {
    let v = validator();
    let d = doc();
    let errors: Vec<String> = v.iter_errors(&d).map(|e| e.to_string()).collect();
    assert!(
        errors.is_empty(),
        "yang catalog failed schema validation:\n{}",
        errors.join("\n")
    );
}

#[test]
fn a_yang_data_entry_in_the_old_schema_node_shape_fails_validation() {
    // patternProperties alone can't make the schema validator reject an
    // unrecognized key shape -- a key matching no pattern is simply
    // skipped, not flagged (there's no additionalProperties/
    // unevaluatedProperties closing yang-data, by deliberate forward-
    // compat policy). So the schema's own pattern must be checked by
    // hand, against a synthetic malformed entry (the real document's
    // own key shape is checked generically by verify_yang_catalog,
    // which to_yang_catalog always runs).
    let v = validator();
    let base = serde_json::json!({
        "$schema": "urn:fercc:yang-catalog-schema:1.0.0",
        "modules": {},
        "import-only-modules": {},
        "typedefs": {},
        "identities": {},
        "schema-nodes": {},
        "sid-index": {}
    });

    // A schema node is not a yang-data template: it lacks "root".
    let schema_node_shaped = serde_json::json!({
        "keyword": "container",
        "name": "error",
        "module": "ietf-coreconf",
        "implicit": false
    });

    let mut with_real_key_shape = base;
    with_real_key_shape["yang-data"] =
        serde_json::json!({ "ietf-coreconf:coreconf-error": schema_node_shaped });
    assert!(
        v.iter_errors(&with_real_key_shape).next().is_some(),
        "a malformed entry under the real \"module:template\" key shape must fail validation"
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
fn identity_entries_carry_real_base() {
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
    assert_ne!(v1["schema-children"], v2["schema-children"]);
}

#[test]
fn the_coreconf_error_template_names_its_root_in_schema_nodes_but_no_module_root_nodes() {
    // ietf-coreconf's `rc:yang-data coreconf-error { container error
    // {...} }` is accept-listed: the template is keyed by its own name,
    // its container lives in schema-nodes at the path its .sid file
    // addresses it by, and nothing about it leaks into the module's
    // datastore schema-root-nodes.
    let d = doc();
    assert_eq!(
        d["yang-data"],
        serde_json::json!({
            "ietf-coreconf:coreconf-error": {
                "module": "ietf-coreconf",
                "name": "coreconf-error",
                "root": "/ietf-coreconf:error"
            }
        })
    );
    let module = &d["modules"]["ietf-coreconf"];
    assert_eq!(
        module["yang-data"],
        serde_json::json!(["ietf-coreconf:coreconf-error"])
    );
    assert_eq!(module["schema-root-nodes"], serde_json::json!([]));
    assert_eq!(module["data-root-nodes"], serde_json::json!([]));
    let error_root = &d["schema-nodes"]["/ietf-coreconf:error"];
    assert_eq!(error_root["keyword"], "container");
    assert!(error_root.get("parent").is_none());
    let sid = error_root["sid"].as_str().unwrap();
    assert_eq!(
        d["sid-index"][sid],
        serde_json::json!({"namespace": "data", "identifier": "/ietf-coreconf:error"})
    );
    assert_eq!(
        d["schema-nodes"]["/ietf-coreconf:error/error-tag"]["parent"],
        "/ietf-coreconf:error"
    );
}

#[test]
fn every_modules_identities_are_exactly_the_values_the_device_implements() {
    // Derived by hand, independently of fercc: each live identityref's
    // value set is the bundled ui-tailoring.yaml's accept-list for its
    // base, or every identity derived from the base when it has none.
    // ieee1588-ptp's clock-class/clock-accuracy and ietf-coreconf's
    // error-tag/error-app-tag have no accept-list and every derived
    // identity is a value, so those are counted rather than listed.
    // Abstract bases are only referred to as bases, never implemented
    // as values, so no implemented module lists them;
    // ieee802-dot1q-types:type-of-operation is the only one whose
    // module is import-only, so it's the only one listed at all.
    const EXPECTED: &[(&str, &[&str])] = &[
        (
            "iana-hardware",
            &[
                "iana-hardware:chassis",
                "iana-hardware:container",
                "iana-hardware:module",
                "iana-hardware:port",
                "iana-hardware:sensor",
            ],
        ),
        (
            "iana-if-type",
            &[
                "iana-if-type:ethernetCsmacd",
                "iana-if-type:ieee8023adLag",
                "iana-if-type:l3ipvlan",
            ],
        ),
        (
            "ieee802-dot1q-bridge",
            &[
                "ieee802-dot1q-bridge:c-vlan-bridge-port",
                "ieee802-dot1q-bridge:d-bridge-port",
                "ieee802-dot1q-bridge:provider-network-port",
            ],
        ),
        (
            "ieee802-dot1q-sched",
            &[
                "ieee802-dot1q-sched:set-and-hold-mac",
                "ieee802-dot1q-sched:set-and-release-mac",
                "ieee802-dot1q-sched:set-gate-states",
            ],
        ),
        (
            "ietf-datastores",
            &[
                "ietf-datastores:operational",
                "ietf-datastores:running",
                "ietf-datastores:startup",
            ],
        ),
        (
            "ietf-igmp-mld-snooping",
            &[
                "ietf-igmp-mld-snooping:igmp-snooping",
                "ietf-igmp-mld-snooping:mld-snooping",
            ],
        ),
        (
            "ietf-ipv4-unicast-routing",
            &["ietf-ipv4-unicast-routing:ipv4-unicast"],
        ),
        (
            "ietf-routing",
            &[
                "ietf-routing:direct",
                "ietf-routing:ipv4",
                "ietf-routing:ipv6",
                "ietf-routing:static",
            ],
        ),
        (
            "mchp-velocitysp-bridge",
            &["mchp-velocitysp-bridge:provider-custom-network-port"],
        ),
        (
            "mchp-velocitysp-ptp",
            &["mchp-velocitysp-ptp:ca-time-accurate-unknown"],
        ),
    ];
    const COUNTED: &[(&str, usize)] = &[("ieee1588-ptp", 37), ("ietf-coreconf", 21)];
    const BASE_ONLY: &[&str] = &[
        "iana-hardware:hardware-class",
        "iana-if-type:iana-interface-type",
        "ieee1588-ptp:clock-accuracy",
        "ieee1588-ptp:clock-class",
        "ieee802-dot1q-bridge:type-of-port",
        "ieee802-dot1q-types:type-of-operation",
        "ietf-coreconf:error-app-tag",
        "ietf-coreconf:error-tag",
        "ietf-datastores:conventional",
        "ietf-datastores:datastore",
        "ietf-interfaces:interface-type",
        "ietf-routing:address-family",
        "ietf-routing:control-plane-protocol",
        "ietf-routing:routing-protocol",
    ];
    let d = doc();
    let mut implemented = std::collections::HashSet::new();
    for (name, module) in d["modules"].as_object().unwrap() {
        let listed: Vec<&str> = module["identities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i.as_str().unwrap())
            .collect();
        implemented.extend(listed.iter().map(|i| i.to_string()));
        if let Some((_, expected)) = EXPECTED.iter().find(|(m, _)| m == name) {
            let mut listed = listed.clone();
            listed.sort();
            assert_eq!(listed, expected.to_vec(), "modules[{name:?}].identities");
        } else if let Some((_, count)) = COUNTED.iter().find(|(m, _)| m == name) {
            assert_eq!(listed.len(), *count, "modules[{name:?}].identities");
            assert!(listed.iter().all(|i| !BASE_ONLY.contains(i)));
        } else {
            assert!(
                listed.is_empty(),
                "modules[{name:?}].identities: {listed:?}"
            );
        }
    }
    let import_only_listed: Vec<&str> = d["import-only-modules"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|m| m["identities"].as_array().unwrap())
        .map(|i| i.as_str().unwrap())
        .collect();
    assert_eq!(
        import_only_listed,
        vec!["ieee802-dot1q-types:type-of-operation"]
    );
    let mut base_only: Vec<&str> = d["identities"]
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .filter(|k| !implemented.contains(*k))
        .collect();
    base_only.sort();
    assert_eq!(base_only, BASE_ONLY.to_vec());
}

#[test]
fn the_bundled_catalogs_type_defaults_units_and_require_instance_reach_their_leaves() {
    // From this catalog's own YANG text: ieee802-ethernet-interface's
    // duplex-type declares "default full" and mchp-velocitysp-system's
    // access-rights "default no-access", and every leaf using them has
    // no default of its own and isn't mandatory, so they take the type's
    // (RFC 7950 7.6.1). ietf-hardware's component/parent is the
    // catalog's only "require-instance false".
    let d = doc();
    assert_eq!(
        d["typedefs"]["ieee802-ethernet-interface:duplex-type"]["default"],
        "full"
    );
    assert_eq!(
        d["typedefs"]["mchp-velocitysp-system:access-rights"]["default"],
        "no-access"
    );
    assert_eq!(
        d["typedefs"]["ieee802-ethernet-interface:eth-if-speed-type"]["units"],
        "Gb/s"
    );
    let nodes = d["schema-nodes"].as_object().unwrap();
    let via = |typedef: &str| -> Vec<&Json> {
        nodes
            .values()
            .filter(|n| {
                n.get("type").and_then(|t| t.get("base-type")) == Some(&Json::from(typedef))
            })
            .collect()
    };
    let duplex = via("ieee802-ethernet-interface:duplex-type");
    assert_eq!(duplex.len(), 2);
    assert!(duplex.iter().all(|n| n["default"] == "full"));
    let rights = via("mchp-velocitysp-system:access-rights");
    assert_eq!(rights.len(), 10);
    assert!(rights.iter().all(|n| n["default"] == "no-access"));
    let last_member = &nodes["/ietf-routing:routing/control-plane-protocols/control-plane-protocol/ietf-igmp-mld-snooping:igmp-snooping-instance/last-member-query-interval"];
    assert_eq!(last_member["units"], "deciseconds");
    assert_eq!(last_member["default"], "10");
    let parent = &nodes["/ietf-hardware:hardware/component/parent"]["type"];
    assert_eq!(parent["require-instance"], false);
}

#[test]
fn every_modules_deviations_match_the_hand_derived_deviation_statements() {
    // Derived by hand from every `deviation` statement in this catalog's
    // own .yang files: each target node's defining module, resolved
    // from the prefix of the path's last segment, not the first --
    // e.g. ietf-interfaces-dev's
    // "/if:interfaces/if:interface/ip:ipv4/ip:neighbor" deviates
    // ietf-ip, not ietf-interfaces. Every implemented module not
    // listed here has no deviations.
    const EXPECTED: &[(&str, &[&str])] = &[
        ("ieee1588-ptp", &["ieee1588-ptp-dev"]),
        ("ieee802-dot1ab-lldp", &["ieee802-dot1ab-lldp-dev"]),
        ("ieee802-dot1as-ptp", &["ieee1588-ptp-dev"]),
        (
            "ieee802-dot1cb-stream-identification",
            &[
                "ieee802-dot1cb-stream-identification-dev",
                "ietf-interfaces-dev",
            ],
        ),
        (
            "ieee802-dot1q-bridge",
            &["ieee802-dot1q-bridge-dev", "ietf-interfaces-dev"],
        ),
        ("ieee802-dot1q-preemption-bridge", &["ietf-interfaces-dev"]),
        ("ieee802-dot1q-psfp-bridge", &["ieee802-dot1q-bridge-dev"]),
        ("ieee802-dot1q-sched-bridge", &["ietf-interfaces-dev"]),
        ("ieee802-ethernet-interface", &["ietf-interfaces-dev"]),
        (
            "ietf-constrained-yang-library",
            &["ietf-constrained-yang-library-dev"],
        ),
        ("ietf-hardware", &["ietf-hardware-dev"]),
        ("ietf-igmp-mld-snooping", &["ietf-igmp-mld-snooping-dev"]),
        ("ietf-interfaces", &["ietf-interfaces-dev"]),
        ("ietf-ip", &["ietf-interfaces-dev"]),
        ("ietf-ipv4-unicast-routing", &["ietf-routing-dev"]),
        ("ietf-netconf-acm", &["ietf-netconf-acm-dev"]),
        ("ietf-routing", &["ietf-routing-dev"]),
        ("ietf-system", &["ietf-system-dev"]),
        ("mchp-velocitysp-port", &["ietf-interfaces-dev"]),
    ];
    let d = doc();
    for (name, module) in d["modules"].as_object().unwrap() {
        let expected = EXPECTED
            .iter()
            .find(|(m, _)| m == name)
            .map(|(_, devs)| *devs)
            .unwrap_or(&[]);
        assert_eq!(
            module["deviations"],
            serde_json::json!(expected),
            "modules[{name:?}].deviations"
        );
    }
    for (name, _) in EXPECTED {
        assert!(
            d["modules"].get(*name).is_some(),
            "{name} must be implemented"
        );
    }
}

#[test]
fn every_module_lists_no_yang_data_except_ietf_coreconf() {
    // ietf-restconf also declares yang-data templates (yang-errors,
    // yang-api), but this device isn't a RESTCONF server, so they are
    // not accept-listed and must not appear anywhere.
    let d = doc();
    for (name, module) in d["modules"].as_object().unwrap() {
        if name != "ietf-coreconf" {
            assert_eq!(
                module["yang-data"],
                serde_json::json!([]),
                "modules[{name:?}].yang-data"
            );
        }
    }
    let schema = schema();
    let names: Vec<String> = schema
        .yang_data()
        .map(|y| format!("{}:{}", y.module, y.name))
        .collect();
    assert_eq!(names, vec!["ietf-coreconf:coreconf-error"]);
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
    // a yang-data root, a wire-addressable SID, or anything the yang
    // catalog reports.
    assert!(schema
        .node_by_schema_path("/ietf-sid-file:sid-file")
        .is_none());
    assert!(schema
        .yang_data()
        .all(|y| y.root().path() != "/ietf-sid-file:sid-file"));
    assert!((29001..=29017).all(|sid| schema.node_by_sid(sid).is_none()));
    let d = doc();
    assert!(d["schema-nodes"].get("/ietf-sid-file:sid-file").is_none());
    assert!(d["yang-data"]
        .as_object()
        .unwrap()
        .values()
        .all(|y| y["root"] != "/ietf-sid-file:sid-file"));
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
    let schema_nodes = d["schema-nodes"].as_object().unwrap();
    let mut checked = 0;
    for template in d["yang-data"].as_object().unwrap().values() {
        let root = template["root"].as_str().unwrap();
        for (path, node) in schema_nodes {
            if path == root || path.starts_with(&format!("{root}/")) {
                checked += 1;
                if let Some(config) = node.get("config") {
                    assert_eq!(config, false, "schema-nodes[{path:?}].config must be false");
                }
            }
        }
    }
    assert!(checked > 1);
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
    // identityref's own base, narrowed by the catalog's own
    // ui-tailoring.yaml identity-accept-list where
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
    // (on ui-tailoring.yaml's yang-data-accept-list), so
    // "ietf-restconf" is listed on
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
    // ieee802-dot1q-bridge's own identities only lists 3 of its 7 real
    // "type-of-port"-derived identities here:
    // "customer-network-port"/"customer-edge-port"/
    // "remote-customer-access-port" aren't on ui-tailoring.yaml's
    // "ieee802-dot1q-bridge:type-of-port" accept-list (the device's
    // "mchp-vlan-port-type" typedef names the same base, so it can only
    // describe that narrowing, not express it: RFC 7950 9.10.1 says an
    // identityref cannot be restricted), so they're unreachable
    // identities entirely, not merely out of declaration order. The
    // abstract base "type-of-port" itself is only referred to as the
    // leaf's base, not a value the device implements, so isn't listed
    // either.
    let d = doc();
    assert_eq!(
        d["modules"]["ieee802-dot1q-bridge"]["identities"],
        serde_json::json!([
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
