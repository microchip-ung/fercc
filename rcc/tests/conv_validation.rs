// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! Validates `rcc::convert` (the core of `rcc conv`) across every
//! YAML/JSON/CBOR direction, against the real fixture corpus in
//! `test-data/` (the same 12 round-trip + 4 error-res fixtures
//! `yang/tests/fixtures.rs` uses). Calls `rcc::convert` directly --
//! deliberately not a subprocess test: spawning the built binary to
//! validate its own conversion logic would mean the logic isn't
//! actually unit-testable, just observable from outside.
//!
//! Ground truth is the real captured `.cbor` bytes: every fixture's
//! `yaml -> cbor` and `json -> cbor` (derived from the fixture's own
//! `cbor -> json`) must equal them exactly. Every other direction
//! (`cbor -> yaml`, `yaml -> yaml`, `json -> yaml`, `json -> json`,
//! `cbor -> cbor` normalize) has no independent ground truth of its own,
//! so it's checked for consistency instead: converting back through
//! `-> json` must always land on the exact same JSON `convert` produces
//! straight from the real `.cbor` bytes. Together these two kinds of
//! check cover all 9 meaningful direction pairs without needing a
//! captured fixture for JSON specifically or for every direction.
//!
//! This suite caught a real bug on first use: decoding a POST/RPC
//! payload didn't re-wrap the result under `input`/`output`, so
//! `cbor -> json` for `flush-mac_table-post-req` didn't match
//! `yaml -> json` for the same fixture -- see `yang/src/codec.rs`'s
//! `decode_node_value` `"rpc" | "action"` arm.

use std::fs;
use std::path::{Path, PathBuf};

use rcc::{content_format_of, convert, ConvInput, ConvOutput};
use yang::codec::ContentFormat;
use yang::schema::Schema;

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("test-data")
}

fn schema() -> Schema {
    let tarball = test_data_dir().join("e6311dd5be50af0f0286fd3a6fb218a1.tar.gz");
    let file = fs::File::open(&tarball).expect("open bundled catalog tarball");
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let mut yang_sources = Vec::new();
    let mut sid_sources = Vec::new();
    for entry in archive.entries().expect("read tar entries") {
        let mut entry = entry.expect("tar entry");
        let path = entry.path().expect("entry path").to_path_buf();
        let mut content = String::new();
        std::io::Read::read_to_string(&mut entry, &mut content).expect("read entry content");
        match path.extension().and_then(|e| e.to_str()) {
            Some("yang") => yang_sources.push(content),
            Some("sid") => sid_sources.push(content),
            _ => {}
        }
    }
    yang::schema::build(&yang_sources, &sid_sources).expect("build schema from bundled catalog")
}

fn text_of(output: ConvOutput, desc: &str) -> String {
    match output {
        ConvOutput::Text(s) => s,
        ConvOutput::Bytes(_) => panic!("{desc}: expected text output, got bytes"),
    }
}

fn bytes_of(output: ConvOutput, desc: &str) -> Vec<u8> {
    match output {
        ConvOutput::Bytes(b) => b,
        ConvOutput::Text(_) => panic!("{desc}: expected byte output, got text"),
    }
}

fn convert_text(schema: &Schema, input_format: &str, output_format: &str, cf: ContentFormat, input: &str, desc: &str) -> String {
    let out = convert(schema, input_format, output_format, cf, false, ConvInput::Text(input)).unwrap_or_else(|e| panic!("{desc}: {e}"));
    text_of(out, desc)
}

fn convert_bytes_to_text(schema: &Schema, input_format: &str, output_format: &str, cf: ContentFormat, input: &[u8], desc: &str) -> String {
    let out = convert(schema, input_format, output_format, cf, false, ConvInput::Bytes(input)).unwrap_or_else(|e| panic!("{desc}: {e}"));
    text_of(out, desc)
}

/// The 12 round-trip fixtures `yang/tests/fixtures.rs` uses, with their
/// content format inferred the same way (`content_format_for`'s suffix
/// rule), spelled out here as the exact `-c` value `rcc conv` takes.
const ROUND_TRIP_FIXTURES: &[(&str, &str)] = &[
    ("1pps-disable-ipatch-req", "ipatch"),
    ("ace-any-drop-ipatch-req", "ipatch"),
    ("add-new-user-ipatch-req", "ipatch"),
    ("flush-mac_table-post-req", "post"),
    ("inner-tag-fetch-res", "fetch"),
    ("lldp-cnt-fetch-req", "fetch"),
    ("mirror-fetch-res", "fetch"),
    ("police-known-multicast-ipatch-req", "ipatch"),
    ("port-conf-mchp-fetch-res", "fetch"),
    ("port-status-all-fetch-res", "fetch"),
    ("ptp-cal-latency-ipatch-req", "ipatch"),
    ("yang-library-fetch-res", "fetch"),
];

#[test]
fn round_trip_fixtures_convert_correctly_in_every_direction() {
    let schema = schema();
    let dir = test_data_dir();
    assert_eq!(ROUND_TRIP_FIXTURES.len(), 12, "yang/tests/fixtures.rs expects exactly 12 -- keep this list in sync with it");

    for &(name, content) in ROUND_TRIP_FIXTURES {
        let cf = content_format_of(content);
        let yaml_text = fs::read_to_string(dir.join(format!("{name}.yaml"))).unwrap_or_else(|e| panic!("{name}: reading fixture yaml: {e}"));
        let expected_cbor = fs::read(dir.join(format!("{name}.cbor"))).unwrap_or_else(|e| panic!("{name}: reading fixture cbor: {e}"));

        // Ground truth #1: yaml -> cbor must equal the real fixture bytes.
        let y2c = bytes_of(
            convert(&schema, "yaml", "cbor", cf, false, ConvInput::Text(&yaml_text)).unwrap_or_else(|e| panic!("{name} yaml->cbor: {e}")),
            &format!("{name} yaml->cbor"),
        );
        assert_eq!(y2c, expected_cbor, "{name}: yaml->cbor did not match the real fixture bytes");

        // The reference JSON view: decoding the real fixture bytes.
        let c2j = convert_bytes_to_text(&schema, "cbor", "json", cf, &expected_cbor, &format!("{name} cbor->json"));

        // yaml->json must agree with cbor->json -- same logical value,
        // two different paths in.
        let y2j = convert_text(&schema, "yaml", "json", cf, &yaml_text, &format!("{name} yaml->json"));
        assert_eq!(y2j, c2j, "{name}: yaml->json != cbor->json");

        // Ground truth #2: json->cbor (using the json derived above)
        // must also equal the real fixture bytes.
        let j2c = bytes_of(
            convert(&schema, "json", "cbor", cf, false, ConvInput::Text(&c2j)).unwrap_or_else(|e| panic!("{name} json->cbor: {e}")),
            &format!("{name} json->cbor"),
        );
        assert_eq!(j2c, expected_cbor, "{name}: json->cbor did not match the real fixture bytes");

        // cbor->yaml, re-parsed back to json, must still agree.
        let c2y = convert_bytes_to_text(&schema, "cbor", "yaml", cf, &expected_cbor, &format!("{name} cbor->yaml"));
        let c2y2j = convert_text(&schema, "yaml", "json", cf, &c2y, &format!("{name} cbor->yaml->json"));
        assert_eq!(c2y2j, c2j, "{name}: cbor->yaml->json != cbor->json");

        // yaml->yaml passthrough, re-parsed back to json, must agree.
        let y2y = convert_text(&schema, "yaml", "yaml", cf, &yaml_text, &format!("{name} yaml->yaml"));
        let y2y2j = convert_text(&schema, "yaml", "json", cf, &y2y, &format!("{name} yaml->yaml->json"));
        assert_eq!(y2y2j, c2j, "{name}: yaml->yaml->json != cbor->json");

        // json->yaml, re-parsed back to json, must agree.
        let j2y = convert_text(&schema, "json", "yaml", cf, &c2j, &format!("{name} json->yaml"));
        let j2y2j = convert_text(&schema, "yaml", "json", cf, &j2y, &format!("{name} json->yaml->json"));
        assert_eq!(j2y2j, c2j, "{name}: json->yaml->json != cbor->json");

        // json->json passthrough must agree.
        let j2j = convert_text(&schema, "json", "json", cf, &c2j, &format!("{name} json->json"));
        assert_eq!(j2j, c2j, "{name}: json->json != cbor->json");

        // cbor->cbor (schema-independent normalize), decoded back, must
        // agree -- and for this already-canonical corpus, is expected to
        // be byte-identical to the original too.
        let c2c = bytes_of(
            convert(&schema, "cbor", "cbor", cf, false, ConvInput::Bytes(&expected_cbor)).unwrap_or_else(|e| panic!("{name} cbor->cbor: {e}")),
            &format!("{name} cbor->cbor"),
        );
        assert_eq!(c2c, expected_cbor, "{name}: cbor->cbor normalize was not byte-identical");
        let c2c2j = convert_bytes_to_text(&schema, "cbor", "json", cf, &c2c, &format!("{name} cbor->cbor->json"));
        assert_eq!(c2c2j, c2j, "{name}: cbor->cbor->json != cbor->json");
    }
}

/// Real device *error* responses (content-format `yang`): decode-only,
/// same set `yang/tests/fixtures.rs`'s `coreconf_error_responses_decode_
/// correctly` uses. No encode ground truth exists for these (a client
/// never constructs one), so only the decode directions are checked.
const ERROR_RES_FIXTURES: &[&str] =
    &["invalid-node-type-error-res", "ipatch-nonexistent-instance-error-res", "missing-list-entry-error-res", "unknown-instance-path-error-res"];

#[test]
fn error_res_fixtures_decode_correctly_in_every_direction() {
    let schema = schema();
    let dir = test_data_dir();
    let cf = ContentFormat::Yang;
    assert_eq!(ERROR_RES_FIXTURES.len(), 4, "yang/tests/fixtures.rs expects exactly 4 -- keep this list in sync with it");

    for &name in ERROR_RES_FIXTURES {
        let yaml_text = fs::read_to_string(dir.join(format!("{name}.yaml"))).unwrap_or_else(|e| panic!("{name}: reading fixture yaml: {e}"));
        let cbor_bytes = fs::read(dir.join(format!("{name}.cbor"))).unwrap_or_else(|e| panic!("{name}: reading fixture cbor: {e}"));

        let c2j = convert_bytes_to_text(&schema, "cbor", "json", cf, &cbor_bytes, &format!("{name} cbor->json"));
        let y2j = convert_text(&schema, "yaml", "json", cf, &yaml_text, &format!("{name} yaml->json"));
        assert_eq!(c2j, y2j, "{name}: cbor->json != yaml->json (the fixture's own expected shape)");

        let c2y = convert_bytes_to_text(&schema, "cbor", "yaml", cf, &cbor_bytes, &format!("{name} cbor->yaml"));
        let c2y2j = convert_text(&schema, "yaml", "json", cf, &c2y, &format!("{name} cbor->yaml->json"));
        assert_eq!(c2y2j, c2j, "{name}: cbor->yaml->json != cbor->json");

        let c2c = bytes_of(
            convert(&schema, "cbor", "cbor", cf, false, ConvInput::Bytes(&cbor_bytes)).unwrap_or_else(|e| panic!("{name} cbor->cbor: {e}")),
            &format!("{name} cbor->cbor"),
        );
        let c2c2j = convert_bytes_to_text(&schema, "cbor", "json", cf, &c2c, &format!("{name} cbor->cbor->json"));
        assert_eq!(c2c2j, c2j, "{name}: cbor->cbor->json != cbor->json");
    }
}
