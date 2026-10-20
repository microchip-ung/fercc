// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value as Json;
use yang::json_schema::to_json_schema_for_catalog;

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test-data")
}

#[test]
fn the_bundled_catalogs_json_schema_matches_the_golden_file() {
    // test-data/json-schema-golden.json is fercc's own JSON Schema for the
    // bundled catalog, ui-tailoring applied. A change to it is a change
    // to the schema fercc produces: review the difference, and if it is
    // intended, regenerate the file with
    //
    //   fercc schema <dir>/*.yang <dir>/ui-tailoring.yaml
    //
    // on the bundled catalog unpacked into <dir>.
    let tarball = test_data_dir().join("9a836fc0bdf2b7b77ba49ea26c94261e.tar.gz");
    let bytes = fs::read(&tarball).expect("read bundled catalog tarball");
    let schema = yang::catalog::load_from_tarball(&bytes)
        .expect("load bundled catalog tarball")
        .build_schema()
        .expect("build schema from bundled catalog");
    let generated = to_json_schema_for_catalog(&schema, "yang").unwrap();

    let golden_text = fs::read_to_string(test_data_dir().join("json-schema-golden.json"))
        .expect("read the golden JSON schema");
    let golden: Json = serde_json::from_str(&golden_text).expect("parse the golden JSON schema");
    assert!(
        generated == golden,
        "fercc's JSON schema for the bundled catalog no longer matches test-data/json-schema-golden.json"
    );
    // Byte for byte, too: the order of keys is part of the output.
    let rendered = serde_json::to_string_pretty(&generated).unwrap();
    assert_eq!(rendered.trim_end(), golden_text.trim_end());
}
