// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use fercc::io::*;
use serde_json::Value as Json;

fn temp_file(name: &str, contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("fercc-io-test-{}-{name}", std::process::id()));
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn parses_json_text() {
    let v = parse(r#"{"a": 1}"#, Format::Json).unwrap();
    assert_eq!(v, serde_json::json!({"a": 1}));
}

#[test]
fn parses_yaml_text() {
    let v = parse("a: 1\n", Format::Yaml).unwrap();
    assert_eq!(v, serde_json::json!({"a": 1}));
}

#[test]
fn renders_json_and_yaml() {
    let v = serde_json::json!({"a": 1});
    assert_eq!(render(&v, Format::Json).unwrap(), "{\n  \"a\": 1\n}");
    assert_eq!(render(&v, Format::Yaml).unwrap(), "a: 1\n");
}

#[test]
fn read_input_uses_explicit_format_flag_over_extension() {
    // A .yaml-named file whose content is actually JSON, forced to
    // parse as JSON via the flag -- flag must win over extension.
    let path = temp_file("explicit-flag.yaml", r#"{"a": 1}"#);
    let (v, fmt) = read_input(Some(path.to_str().unwrap()), Some("json")).unwrap();
    assert_eq!(fmt, Format::Json);
    assert_eq!(v, serde_json::json!({"a": 1}));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn read_input_infers_json_from_extension_when_no_flag_given() {
    let path = temp_file("inferred.json", r#"{"b": 2}"#);
    let (v, fmt) = read_input(Some(path.to_str().unwrap()), None).unwrap();
    assert_eq!(fmt, Format::Json);
    assert_eq!(v, serde_json::json!({"b": 2}));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn read_input_defaults_to_yaml_with_no_flag_or_recognized_extension() {
    let path = temp_file("no-hint.txt", "c: 3\n");
    let (v, fmt) = read_input(Some(path.to_str().unwrap()), None).unwrap();
    assert_eq!(fmt, Format::Yaml);
    assert_eq!(v, serde_json::json!({"c": 3}));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn write_output_infers_json_from_extension_when_no_flag_given() {
    let path = temp_file("out.json", "");
    write_output(
        &serde_json::json!({"d": 4}),
        Some(path.to_str().unwrap()),
        None,
    )
    .unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        parse(&written, Format::Json).unwrap(),
        serde_json::json!({"d": 4})
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn write_output_explicit_format_flag_overrides_extension() {
    let path = temp_file("out.yaml", "");
    write_output(
        &serde_json::json!({"e": 5}),
        Some(path.to_str().unwrap()),
        Some("json"),
    )
    .unwrap();
    let written = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        parse(&written, Format::Json).unwrap(),
        serde_json::json!({"e": 5})
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn write_output_skips_empty_values() {
    let path = temp_file("should-stay-empty.json", "");
    write_output(&Json::Null, Some(path.to_str().unwrap()), None).unwrap();
    write_output(&serde_json::json!([]), Some(path.to_str().unwrap()), None).unwrap();
    write_output(&serde_json::json!({}), Some(path.to_str().unwrap()), None).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    std::fs::remove_file(path).unwrap();
}
