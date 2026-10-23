// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use yang::parser::*;

#[test]
fn parses_a_minimal_module() {
    let src = r#"
        module foo {
          namespace "urn:foo";
          prefix "f";

          container bar {
            leaf baz {
              type string;
            }
          }
        }
    "#;
    let m = parse_module(src).unwrap();
    assert_eq!(m.keyword, "module");
    assert_eq!(m.arg_str(), "foo");
    assert_eq!(m.sub("namespace").unwrap().arg_str(), "urn:foo");
    let bar = m.sub("container").unwrap();
    assert_eq!(bar.arg_str(), "bar");
    let baz = bar.sub("leaf").unwrap();
    assert_eq!(baz.arg_str(), "baz");
    assert_eq!(baz.sub("type").unwrap().arg_str(), "string");
}

#[test]
fn handles_string_concatenation_and_comments() {
    let src = r#"
        // a line comment
        module foo {
          namespace "urn:"
            + "foo"; /* a block comment */
          prefix f;
        }
    "#;
    let m = parse_module(src).unwrap();
    assert_eq!(m.sub("namespace").unwrap().arg_str(), "urn:foo");
}

#[test]
fn handles_single_quoted_strings_without_escapes() {
    let src = r#"module foo { pattern '[0-9\.]*'; }"#;
    let m = parse_module(src).unwrap();
    assert_eq!(m.sub("pattern").unwrap().arg_str(), "[0-9\\.]*");
}

#[test]
fn parses_extension_statements_with_a_prefix() {
    let src = r#"
        module foo {
          rc:yang-data coreconf-error {
            container error;
          }
        }
    "#;
    let m = parse_module(src).unwrap();
    let ext = &m.subs[0];
    assert_eq!(ext.prefix.as_deref(), Some("rc"));
    assert_eq!(ext.keyword, "yang-data");
    assert_eq!(ext.arg_str(), "coreconf-error");
}

#[test]
fn rejects_non_module_top_level_statement() {
    let src = "leaf foo { type string; }";
    assert!(parse_module(src).is_err());
}
