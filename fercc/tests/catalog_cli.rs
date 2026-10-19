// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value as Json;

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test-data")
}

fn unique_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fercc-catalog-cli-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn extract_bundled_catalog(dest: &Path) -> Vec<String> {
    let tarball = test_data_dir().join("9a836fc0bdf2b7b77ba49ea26c94261e.tar.gz");
    let gz = flate2::read::GzDecoder::new(fs::File::open(tarball).unwrap());
    let mut archive = tar::Archive::new(gz);
    let mut files = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().into_owned();
        if path.components().count() != 1 {
            continue;
        }
        let out = dest.join(&path);
        entry.unpack(&out).unwrap();
        files.push(out.to_str().unwrap().to_string());
    }
    files
}

fn fercc(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_fercc"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn catalog_generates_the_same_yang_catalog_to_a_file_and_to_stdout() {
    let dir = unique_dir("generate");
    let files = extract_bundled_catalog(&dir);
    let out = dir.join("yang-catalog.json");
    let mut args = vec!["catalog", "-o", out.to_str().unwrap()];
    args.extend(files.iter().map(String::as_str));
    let written = fercc(&args);
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    let from_file = fs::read(&out).unwrap();

    let mut args = vec!["catalog"];
    args.extend(files.iter().map(String::as_str));
    let printed = fercc(&args);
    assert!(
        printed.status.success(),
        "{}",
        String::from_utf8_lossy(&printed.stderr)
    );
    assert_eq!(printed.stdout, from_file);

    let doc: Json = serde_json::from_slice(&from_file).unwrap();
    assert_eq!(doc["$schema"], "urn:fercc:yang-catalog-schema:1.0.0");
    assert!(doc.get("yang-catalog").is_none());
    // The bundled ui-tailoring.yaml is among the files, so its
    // per-leaf accept-list must have been applied.
    assert_eq!(
        doc["schema-nodes"]["/ietf-routing:routing/ribs/rib/address-family"]["type"]["values"],
        serde_json::json!(["ietf-routing:ipv4"])
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn catalog_output_does_not_depend_on_the_order_of_its_files() {
    let dir = unique_dir("order");
    let mut files = extract_bundled_catalog(&dir);
    files.sort();
    let mut args = vec!["catalog"];
    args.extend(files.iter().map(String::as_str));
    let forward = fercc(&args);
    files.reverse();
    let mut args = vec!["catalog"];
    args.extend(files.iter().map(String::as_str));
    let backward = fercc(&args);
    assert!(forward.status.success() && backward.status.success());
    assert_eq!(forward.stdout, backward.stdout);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn catalog_rejects_a_file_that_is_not_a_catalog_source() {
    let dir = unique_dir("reject");
    let stray = dir.join("data.json");
    fs::write(&stray, "{}").unwrap();
    let result = fercc(&["catalog", stray.to_str().unwrap()]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("unexpected argument(s)"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn catalog_requires_at_least_one_yang_file() {
    let dir = unique_dir("no-yang");
    let tailoring = dir.join("ui-tailoring.yaml");
    fs::write(&tailoring, "ui-tailoring:\n  identity-accept-list: []\n").unwrap();
    let result = fercc(&["catalog", tailoring.to_str().unwrap()]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("at least one .yang file"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}
