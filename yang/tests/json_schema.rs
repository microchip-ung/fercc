// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use serde_json::Value as Json;
use yang::json_schema::{to_json_schema, to_json_schema_for_catalog};

#[test]
fn a_put_leaves_out_state_data_but_keeps_an_actions_input_parameters() {
    // ro is config false, so it is not part of a PUT. An action's
    // input parameters have no config at all, so they always are.
    let src = r#"
        module st {
          namespace "urn:st";
          prefix s;

          container box {
            leaf rw {
              type string;
            }
            leaf ro {
              config false;
              type string;
            }
            leaf-list tags {
              type string;
            }
            action reset {
              input {
                leaf delay {
                  type uint8;
                }
                leaf-list ports {
                  type string;
                }
              }
            }
          }
        }
    "#;
    let schema = yang::schema::build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let properties = |node: &str, content_format: &str| -> Vec<String> {
        let json =
            to_json_schema(schema.node_by_schema_path(node).unwrap(), content_format).unwrap();
        json["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    };
    assert_eq!(properties("/st:box", "yang"), ["rw", "ro", "tags"]);
    assert_eq!(properties("/st:box", "put"), ["rw", "tags"]);
    assert_eq!(properties("/st:box/reset/input", "put"), ["delay", "ports"]);

    // uniqueItems is only for configuration, so a leaf-list in an
    // input doesn't get it.
    let unique_items = |node: &str| {
        to_json_schema(schema.node_by_schema_path(node).unwrap(), "yang")
            .unwrap()
            .get("uniqueItems")
            .cloned()
    };
    assert_eq!(unique_items("/st:box/tags"), Some(Json::Bool(true)));
    assert_eq!(unique_items("/st:box/reset/input/ports"), None);

    // The same holds at the top level of the whole schema.
    let src = r#"
        module top {
          namespace "urn:top";
          prefix t;

          leaf rw {
            type string;
          }
          leaf ro {
            config false;
            type string;
          }
        }
    "#;
    let schema = yang::schema::build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let top_level = |content_format: &str| -> Vec<String> {
        to_json_schema_for_catalog(&schema, content_format).unwrap()["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    };
    assert_eq!(top_level("yang"), ["top:rw", "top:ro"]);
    assert_eq!(top_level("put"), ["top:rw"]);
}

#[test]
fn a_bound_that_does_not_fit_in_a_json_number_is_an_error() {
    // 2^64 - 2 bytes fits in a u64, but its base64 length is 4/3 of
    // that and doesn't.
    let src = r#"
        module big {
          namespace "urn:big";
          prefix b;

          leaf blob {
            type binary {
              length "18446744073709551614..max";
            }
          }
        }
    "#;
    let schema = yang::schema::build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
    let err = to_json_schema_for_catalog(&schema, "yang").unwrap_err();
    assert!(err.contains("/big:blob"), "{err}");
    assert!(err.contains("24595658764946068820"), "{err}");
}
