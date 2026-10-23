// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use cbor::Value as Cbor;
use serde_json::Value as Json;
use yang::codec::*;
use yang::schema::*;

#[test]
fn a_put_only_skips_the_required_check_for_state_data() {
    // A mandatory config false leaf may be left out of a PUT, but a
    // mandatory leaf under an action's input has no config at all,
    // so it must still be given.
    let yang = r#"
        module st {
            namespace "urn:st";
            prefix s;

            container box {
                leaf counter {
                    config false;
                    mandatory true;
                    type uint32;
                }
                action reset {
                    input {
                        leaf delay {
                            mandatory true;
                            type uint8;
                        }
                    }
                }
            }
        }
    "#;
    let sid = r#"{
        "module-name": "st",
        "module-revision": "2026-01-01",
        "items": [
            {"namespace": "module", "identifier": "st", "sid": 2000},
            {"namespace": "data", "identifier": "/st:box", "sid": 2001},
            {"namespace": "data", "identifier": "/st:box/counter", "sid": 2002},
            {"namespace": "data", "identifier": "/st:box/reset", "sid": 2003},
            {"namespace": "data", "identifier": "/st:box/reset/input", "sid": 2004},
            {"namespace": "data", "identifier": "/st:box/reset/input/delay", "sid": 2005}
        ]
    }"#;
    let schema =
        yang::schema::build(&[yang.to_string()], &[sid.to_string()], &[], &[], &[]).unwrap();
    let empty = Json::Object(serde_json::Map::new());

    let box_id = schema.node_by_data_path("/st:box").unwrap();
    encode_node_value(&schema, box_id, &empty, ContentFormat::Put, false).unwrap();
    let e = encode_node_value(&schema, box_id, &empty, ContentFormat::Yang, false).unwrap_err();
    assert!(e.0.contains("\"counter\""), "{}", e.0);

    let input = schema.node_by_schema_path("/st:box/reset/input").unwrap();
    let e = encode_node_value(&schema, input, &empty, ContentFormat::Put, false).unwrap_err();
    assert!(e.0.contains("\"delay\""), "{}", e.0);
    assert!(e.0.contains("required"), "{}", e.0);
}

fn cbor_as_i128(value: &Cbor) -> Option<i128> {
    match value {
        Cbor::Integer(i) => Some(*i),
        _ => None,
    }
}

// Ported from support/yang-enc/spec/instance_entry_spec.rb: exact
// error-message wording for a malformed fetch/ipatch/post entry.
// These are validation-only checks (fail before any schema/SID
// lookup), so an empty schema is enough -- and they cover the error
// paths the success-oriented devclient fixture corpus never
// exercises (see yang/tests/fixtures.rs).

fn empty_schema() -> Schema {
    yang::schema::build(&[], &[], &[], &[], &[]).expect("empty schema builds")
}

#[test]
fn rejects_a_non_map_array_entry() {
    let schema = empty_schema();
    let err = json_seq_to_cbor(
        &schema,
        &[Json::Array(vec![Json::String("/some/path".into())])],
        ContentFormat::Ipatch,
        false,
    )
    .unwrap_err();
    assert!(err.0.contains("IPATCH"), "{}", err.0);
    assert!(err.0.contains("got array"), "{}", err.0);
}

#[test]
fn rejects_a_scalar_entry() {
    let schema = empty_schema();
    let err = json_seq_to_cbor(
        &schema,
        &[Json::String("/some/path".into())],
        ContentFormat::Post,
        false,
    )
    .unwrap_err();
    assert!(err.0.contains("POST"), "{}", err.0);
    assert!(err.0.contains("got String"), "{}", err.0);
}

#[test]
fn rejects_a_null_entry() {
    let schema = empty_schema();
    let err = json_seq_to_cbor(&schema, &[Json::Null], ContentFormat::Ipatch, false).unwrap_err();
    assert!(err.0.contains("got null"), "{}", err.0);
}

#[test]
fn rejects_an_entry_with_two_keys_listing_them() {
    let schema = empty_schema();
    let mut map = serde_json::Map::new();
    map.insert("/a".to_string(), Json::from(1));
    map.insert("/b".to_string(), Json::from(2));
    let err =
        json_seq_to_cbor(&schema, &[Json::Object(map)], ContentFormat::Ipatch, false).unwrap_err();
    assert!(err.0.contains("exactly one key/value pair"), "{}", err.0);
    assert!(err.0.contains("got 2"), "{}", err.0);
    assert!(err.0.contains("/a") && err.0.contains("/b"), "{}", err.0);
}

#[test]
fn rejects_an_empty_map() {
    let schema = empty_schema();
    let err = json_seq_to_cbor(
        &schema,
        &[Json::Object(serde_json::Map::new())],
        ContentFormat::Fetch,
        false,
    )
    .unwrap_err();
    assert!(err.0.contains("got 0"), "{}", err.0);
}

// -- native validation / `--continue` leniency --------------------
//
// This is the coverage `json_validate`/`json_schemer` gave the Ruby
// reference (see `type_to_cbor`'s own doc comment for why it's
// native here instead): a hand-built catalog with a range, a length
// + pattern, an `empty` leaf, and a keyed list, pinned independent
// of whatever any real catalog happens to declare. Each check is
// exercised both ways: a hard error without `--continue`, and a
// warning-plus-best-effort encoding with it.

fn validation_test_schema() -> Schema {
    let yang = r#"
        module test {
            namespace "urn:test";
            prefix t;

            container box {
                leaf level {
                    type uint8 {
                        range "0..10";
                    }
                    mandatory true;
                }
                leaf mode {
                    type string {
                        length "1..4";
                        pattern "[a-z]+";
                    }
                }
                leaf armed {
                    type empty;
                }
                leaf-list tag {
                    type string;
                }
                leaf setting {
                    type union {
                        type boolean;
                        type uint8 {
                            range "0..10";
                        }
                    }
                }
                list item {
                    key "id";
                    leaf id {
                        type uint8;
                    }
                    leaf name {
                        type string;
                    }
                }
            }
        }
    "#;
    let sid = r#"{
        "module-name": "test",
        "module-revision": "2026-01-01",
        "items": [
            {"namespace": "module", "identifier": "test", "sid": 1000},
            {"namespace": "data", "identifier": "/test:box", "sid": 1001},
            {"namespace": "data", "identifier": "/test:box/level", "sid": 1002},
            {"namespace": "data", "identifier": "/test:box/mode", "sid": 1003},
            {"namespace": "data", "identifier": "/test:box/armed", "sid": 1004},
            {"namespace": "data", "identifier": "/test:box/item", "sid": 1005},
            {"namespace": "data", "identifier": "/test:box/item/id", "sid": 1006},
            {"namespace": "data", "identifier": "/test:box/item/name", "sid": 1007},
            {"namespace": "data", "identifier": "/test:box/tag", "sid": 1008},
            {"namespace": "data", "identifier": "/test:box/setting", "sid": 1009}
        ]
    }"#;
    yang::schema::build(&[yang.to_string()], &[sid.to_string()], &[], &[], &[])
        .expect("validation test schema builds")
}

fn box_node(schema: &Schema) -> NodeRef<'_> {
    schema
        .node_by_data_path("/test:box")
        .expect("test:box exists")
}

/// A minimally-valid `box` body: just the one mandatory field.
fn box_obj() -> serde_json::Map<String, Json> {
    let mut m = serde_json::Map::new();
    m.insert("level".to_string(), Json::from(5));
    m
}

#[test]
fn range_violation_hard_fails_without_continue_and_degrades_with_it() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    let mut obj = box_obj();
    obj.insert("level".to_string(), Json::from(300));

    let e = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj.clone()),
        ContentFormat::Put,
        false,
    )
    .unwrap_err();
    assert!(e.0.contains("300"), "{}", e.0);
    assert!(e.0.contains("between 0 and 10"), "{}", e.0);

    // Continuing: the request still encodes (degraded, but present).
    assert!(encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj),
        ContentFormat::Put,
        true
    )
    .is_ok());
}

#[test]
fn pattern_violation_hard_fails_without_continue_and_degrades_with_it() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    let mut obj = box_obj();
    obj.insert("mode".to_string(), Json::String("ABC".into()));

    let e = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj.clone()),
        ContentFormat::Put,
        false,
    )
    .unwrap_err();
    assert!(e.0.contains("ABC"), "{}", e.0);
    assert!(e.0.contains("[a-z]+"), "{}", e.0);

    assert!(encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj),
        ContentFormat::Put,
        true
    )
    .is_ok());
}

#[test]
fn length_violation_is_rejected_with_the_actual_bound_shown() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    let mut obj = box_obj();
    obj.insert("mode".to_string(), Json::String("toolong".into()));

    let e = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj),
        ContentFormat::Put,
        false,
    )
    .unwrap_err();
    assert!(e.0.contains("7 character"), "{}", e.0);
    assert!(e.0.contains("between 1 and 4"), "{}", e.0);
}

#[test]
fn unknown_field_hard_fails_without_continue_and_is_skipped_with_it() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    let mut obj = box_obj();
    obj.insert("nope".to_string(), Json::from(1));

    let e = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj.clone()),
        ContentFormat::Put,
        false,
    )
    .unwrap_err();
    assert!(e.0.contains("\"nope\""), "{}", e.0);
    assert!(e.0.contains("not a valid field"), "{}", e.0);

    let cbor = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj),
        ContentFormat::Put,
        true,
    )
    .unwrap();
    let decoded = decode_node_value(&schema, box_id, &cbor, ContentFormat::Put).unwrap();
    let decoded_obj = decoded.as_object().unwrap();
    assert!(
        !decoded_obj.contains_key("nope"),
        "unknown field should have been dropped, got {decoded:?}"
    );
    assert!(decoded_obj.contains_key("level"));
}

#[test]
fn missing_mandatory_field_hard_fails_without_continue_and_is_absent_with_it() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    let obj = serde_json::Map::new(); // no "level" at all

    let e = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj.clone()),
        ContentFormat::Put,
        false,
    )
    .unwrap_err();
    assert!(e.0.contains("\"level\""), "{}", e.0);
    assert!(e.0.contains("required"), "{}", e.0);

    let cbor = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj),
        ContentFormat::Put,
        true,
    )
    .unwrap();
    let decoded = decode_node_value(&schema, box_id, &cbor, ContentFormat::Put).unwrap();
    assert_eq!(
        decoded,
        Json::Object(serde_json::Map::new()),
        "the missing field should simply stay absent, not appear as null or a default"
    );
}

#[test]
fn missing_list_key_hard_fails_without_continue_and_is_absent_with_it() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    let mut item = serde_json::Map::new();
    item.insert("name".to_string(), Json::String("x".into()));
    let mut obj = box_obj();
    obj.insert("item".to_string(), Json::Array(vec![Json::Object(item)]));

    let e = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj.clone()),
        ContentFormat::Put,
        false,
    )
    .unwrap_err();
    assert!(e.0.contains("\"id\""), "{}", e.0);
    assert!(e.0.contains("key of this list"), "{}", e.0);

    assert!(encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj),
        ContentFormat::Put,
        true
    )
    .is_ok());
}

#[test]
fn malformed_leaf_value_hard_fails_without_continue_and_passes_through_raw_with_it() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    let mut obj = box_obj();
    obj.insert("level".to_string(), Json::String("not-a-number".into()));

    let e = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj.clone()),
        ContentFormat::Put,
        false,
    )
    .unwrap_err();
    assert!(e.0.contains("expected a whole number"), "{}", e.0);

    let cbor = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj),
        ContentFormat::Put,
        true,
    )
    .unwrap();
    // Degraded: the raw string made it onto the wire verbatim (delta-
    // SID 1 == level's SID 1002 minus box's own SID 1001), not a
    // properly-typed integer.
    let map = cbor.as_map().expect("still a CBOR map");
    let (_, level_cbor) = map
        .iter()
        .find(|(k, _)| cbor_as_i128(k) == Some(1))
        .expect("level's entry is present");
    assert_eq!(level_cbor, &Cbor::Text("not-a-number".to_string()));
}

#[test]
fn empty_typed_leaf_requires_the_null_array_shape() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    let mut obj = box_obj();
    obj.insert("armed".to_string(), Json::Bool(true)); // wrong shape

    let e = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj.clone()),
        ContentFormat::Put,
        false,
    )
    .unwrap_err();
    assert!(e.0.contains("[null]"), "{}", e.0);

    obj.insert("armed".to_string(), Json::Array(vec![Json::Null])); // correct shape
    assert!(encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj),
        ContentFormat::Put,
        false
    )
    .is_ok());
}

// -- structural shape checks (container -> object, list -> array,
// leaf-list -> array) -- what a JSON-Schema `type` keyword would
// enforce, confirming the encoder's own type-shape checks give at
// least the same coverage without one.

#[test]
fn container_shape_mismatch_hard_fails_without_continue_and_is_empty_with_it() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);

    let e =
        encode_node_value(&schema, box_id, &Json::from(5), ContentFormat::Put, false).unwrap_err();
    assert!(e.0.contains("needs an object"), "{}", e.0);

    // Continuing: `box` itself becomes an empty map -- matches
    // Ruby's own `result = {}` fallback, and the *same* shortcut it
    // takes: `encode_body`'s own missing-required-field check is
    // never reached here, since there's no body to check fields of.
    assert_eq!(
        encode_node_value(&schema, box_id, &Json::from(5), ContentFormat::Put, true).unwrap(),
        Cbor::Map(vec![])
    );
}

#[test]
fn list_shape_mismatch_hard_fails_without_continue_and_is_empty_with_it() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    let item_id = box_id.find_child("item").expect("test:box/item exists");

    let e =
        encode_node_value(&schema, item_id, &Json::from(5), ContentFormat::Put, false).unwrap_err();
    assert!(e.0.contains("needs a list"), "{}", e.0);

    assert_eq!(
        encode_node_value(&schema, item_id, &Json::from(5), ContentFormat::Put, true).unwrap(),
        Cbor::Array(vec![])
    );
}

#[test]
fn leaf_list_shape_mismatch_is_always_a_hard_error_regardless_of_continue() {
    // Unlike container/list, `leaf-list`'s own shape check has no
    // `--continue` leniency in Ruby either (an unguarded `json.map`
    // that would just crash on the wrong shape) -- replicated here
    // faithfully rather than "fixed", per this port's own stated
    // policy (see `encode_node_value`'s doc comment).
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    let tag_id = box_id.find_child("tag").expect("test:box/tag exists");

    assert!(encode_node_value(&schema, tag_id, &Json::from(5), ContentFormat::Put, false).is_err());
    assert!(encode_node_value(&schema, tag_id, &Json::from(5), ContentFormat::Put, true).is_err());
}

#[test]
fn union_mismatch_names_the_value_and_the_allowed_types_not_an_internal_id() {
    let schema = validation_test_schema();
    let box_id = box_node(&schema);
    // `setting` is a `union { boolean; uint8 { range "0..10"; } }` --
    // a plain string matches neither member.
    let mut obj = box_obj();
    obj.insert("setting".to_string(), Json::String("nope".into()));

    let e = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj.clone()),
        ContentFormat::Put,
        false,
    )
    .unwrap_err();
    assert!(e.0.contains("\"nope\""), "{}", e.0);
    assert!(e.0.contains("boolean"), "{}", e.0);
    assert!(e.0.contains("uint8"), "{}", e.0);
    // The old message named an internal TypeId (a bare integer) --
    // never let one leak back in.
    assert!(
        !e.0.contains("type 0") && !e.0.contains("type 1"),
        "should not mention an internal type id: {}",
        e.0
    );

    // A value that *does* match a member still degrades sensibly
    // under --continue if that member's own constraints reject it
    // (300 doesn't fit uint8 0..10).
    obj.insert("setting".to_string(), Json::from(300));
    let e2 = encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj.clone()),
        ContentFormat::Put,
        false,
    )
    .unwrap_err();
    assert!(e2.0.contains("between 0 and 10"), "{}", e2.0);
    assert!(encode_node_value(
        &schema,
        box_id,
        &Json::Object(obj),
        ContentFormat::Put,
        true
    )
    .is_ok());
}

const ROUTING_LIKE: &str = r#"
    module rt {
      namespace "urn:rt";
      prefix rt;

      identity protocol;
      identity routing-protocol {
        base protocol;
      }
      identity direct {
        base routing-protocol;
      }
      identity static {
        base routing-protocol;
      }
      identity snooping {
        base protocol;
      }

      leaf protocol-type {
        type identityref {
          base protocol;
        }
      }
      leaf source-protocol {
        type identityref {
          base routing-protocol;
        }
      }
    }
"#;

#[test]
fn an_identityref_rejects_an_intermediate_identity_its_accept_list_leaves_out() {
    // The codec's set of encodable values is the base's value set, so
    // an intermediate kept only as a base must be rejected, while an
    // accepted identity still encodes and decodes.
    let accepted: &[&str] = &["rt:static", "rt:snooping"];
    let sid = r#"{
        "module-name": "rt",
        "module-revision": "2026-01-01",
        "items": [
            {"namespace": "module", "identifier": "rt", "sid": 1000},
            {"namespace": "data", "identifier": "/rt:protocol-type", "sid": 1001},
            {"namespace": "data", "identifier": "/rt:source-protocol", "sid": 1002},
            {"namespace": "identity", "identifier": "protocol", "sid": 1010},
            {"namespace": "identity", "identifier": "routing-protocol", "sid": 1011},
            {"namespace": "identity", "identifier": "direct", "sid": 1012},
            {"namespace": "identity", "identifier": "static", "sid": 1013},
            {"namespace": "identity", "identifier": "snooping", "sid": 1014}
        ]
    }"#;
    let schema = yang::schema::build(
        &[ROUTING_LIKE.to_string()],
        &[sid.to_string()],
        &[],
        &[("rt:protocol", accepted)],
        &[],
    )
    .unwrap();
    let ty = schema
        .node_by_schema_path("/rt:protocol-type")
        .unwrap()
        .ty()
        .unwrap();
    let encoded = type_to_cbor(&schema, ty, &serde_json::json!("rt:static"), false, false).unwrap();
    assert_eq!(
        type_to_json(&schema, ty, &encoded, false).unwrap(),
        serde_json::json!("rt:static")
    );
    assert!(type_to_cbor(
        &schema,
        ty,
        &serde_json::json!("rt:routing-protocol"),
        false,
        false
    )
    .is_err());
    assert!(type_to_cbor(&schema, ty, &serde_json::json!("rt:direct"), false, false).is_err());
}

#[test]
fn a_string_must_not_match_an_inverted_pattern() {
    // RFC 7950 9.4.6: with "modifier invert-match" the type is
    // restricted to values that do not match the pattern.
    let src = r#"
        module pat {
          namespace "urn:pat";
          prefix p;

          leaf name {
            type string {
              pattern "[a-z]+";
              pattern "x.*" {
                modifier invert-match;
              }
            }
          }
        }
    "#;
    let schema = yang::schema::build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let ty = schema
        .node_by_schema_path("/pat:name")
        .unwrap()
        .ty()
        .unwrap();
    assert!(type_to_cbor(&schema, ty, &serde_json::json!("abc"), false, false).is_ok());
    let e = type_to_cbor(&schema, ty, &serde_json::json!("xyz"), false, false)
        .unwrap_err()
        .to_string();
    assert!(e.contains("not x.*"), "{e}");
    assert!(type_to_cbor(&schema, ty, &serde_json::json!("ABC"), false, false).is_err());
}

#[test]
fn a_violated_restriction_reports_the_yang_authors_error_message() {
    let src = r#"
        module emsg {
          namespace "urn:emsg";
          prefix e;

          leaf level {
            type uint8 {
              range "0..100" {
                error-message "must be a percentage";
                error-app-tag "not-a-percentage";
              }
            }
          }
          leaf name {
            type string {
              length "1..8";
              pattern "[a-z]+" {
                error-message "lower case only";
              }
              pattern "x.*" {
                modifier invert-match;
                error-app-tag "starts-with-x";
              }
            }
          }
        }
    "#;
    let schema = yang::schema::build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let encode_err = |path: &str, value: serde_json::Value| -> String {
        let ty = schema.node_by_schema_path(path).unwrap().ty().unwrap();
        type_to_cbor(&schema, ty, &value, false, false)
            .unwrap_err()
            .to_string()
    };
    assert_eq!(
        encode_err("/emsg:level", serde_json::json!(101)),
        "must be a percentage (not-a-percentage)"
    );
    assert_eq!(
        encode_err("/emsg:name", serde_json::json!("ABC")),
        "lower case only"
    );
    let e = encode_err("/emsg:name", serde_json::json!("xyz"));
    assert!(e.ends_with("(starts-with-x)"), "{e}");
    assert!(e.contains("not x.*"), "{e}");
    let e = encode_err("/emsg:name", serde_json::json!("abcdefghij"));
    assert!(e.contains("10 character(s) long"), "{e}");
}
