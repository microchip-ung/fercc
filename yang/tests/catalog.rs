// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use std::fs;
use std::path::{Path, PathBuf};

use yang::catalog::*;

#[cfg(unix)]
fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test-data")
}

#[cfg(unix)]
fn unique_path(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("fercc-catalog-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    let _ = fs::remove_file(&path);
    path
}

#[test]
fn load_from_dir_leaves_ui_tailoring_none_when_the_file_is_absent() {
    let dir = unique_path("no-ui-tailoring");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("foo.yang"),
        "module foo { namespace \"urn:foo\"; prefix f; }",
    )
    .unwrap();

    let files = load_from_dir(&dir).unwrap();
    assert!(files.ui_tailoring.is_none());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn load_workspace_reads_ui_tailoring_when_present_and_leaves_it_none_when_absent() {
    let repo_root = unique_path("workspace-ui-tailoring");
    let yang_dir = repo_root.join("docs/sw_refs/yang");
    fs::create_dir_all(&yang_dir).unwrap();
    fs::create_dir_all(repo_root.join("support/scripts")).unwrap();
    fs::write(
        repo_root.join("support/scripts/gen-cc-nodes.yaml"),
        "sid-files:\n- foo@2024-01-01.sid\n",
    )
    .unwrap();
    fs::write(
        yang_dir.join("foo.yang"),
        "module foo { namespace \"urn:foo\"; prefix f; }",
    )
    .unwrap();
    fs::write(
        yang_dir.join("foo@2024-01-01.sid"),
        r#"{"module": "foo", "revision": "2024-01-01", "identifier": "foo", "entries": []}"#,
    )
    .unwrap();

    let files = load_workspace(&repo_root).unwrap();
    assert!(files.ui_tailoring.is_none());

    fs::write(
        yang_dir.join("ui-tailoring.yaml"),
        "ui-tailoring:\n  identity-accept-list: []\n",
    )
    .unwrap();
    let files = load_workspace(&repo_root).unwrap();
    assert_eq!(
        files.ui_tailoring.unwrap(),
        "ui-tailoring:\n  identity-accept-list: []\n"
    );

    let _ = fs::remove_dir_all(&repo_root);
}

#[cfg(unix)]
#[test]
fn external_fetcher_contract_end_to_end() {
    // A fake `--catalog-fetcher`: ignores the checksum it's given
    // and just cats the bundled real catalog tarball to stdout --
    // exactly the contract a real replacement is expected to
    // satisfy, and exactly what the built-in `curl` default's own
    // stdout capture must handle too.
    let tarball = test_data_dir().join("9a836fc0bdf2b7b77ba49ea26c94261e.tar.gz");
    let script_path = unique_path("fetcher-script.sh");
    {
        // sync_all() ensures the kernel write lock is released before we
        // exec the script; without it we get ETXTBSY on Linux in CI.
        use std::io::Write;
        let mut f = fs::File::create(&script_path).unwrap();
        f.write_all(format!("#!/bin/sh\nexec cat '{}'\n", tarball.display()).as_bytes())
            .unwrap();
        f.sync_all().unwrap();
    }
    let mut perms = fs::metadata(&script_path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&script_path, perms).unwrap();

    let dest_dir = unique_path("dest");
    download_and_extract(
        "ignored-checksum",
        &dest_dir,
        Some(script_path.to_str().unwrap()),
        false,
    )
    .unwrap();

    let files = load_from_dir(&dest_dir).unwrap();
    assert!(!files.yang.is_empty());
    assert!(!files.sid.is_empty());
    assert!(files
        .ui_tailoring
        .is_some_and(|t| t.contains("identity-accept-list")));
    assert!(dest_dir.join(".complete").is_file());

    // A repeat call must be short-circuited by the `.complete`
    // sentinel before ever invoking the fetcher again -- point at
    // a command that would fail loudly if it were actually run.
    download_and_extract(
        "ignored-checksum",
        &dest_dir,
        Some("this-program-does-not-exist-anywhere"),
        false,
    )
    .unwrap();

    let _ = fs::remove_dir_all(&dest_dir);
    let _ = fs::remove_file(&script_path);
}
