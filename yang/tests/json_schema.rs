// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use yang::json_schema::to_json_schema_for_catalog;

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
