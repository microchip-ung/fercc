// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! YANG catalog acquisition: either this repo's checked-out sources
//! (`--workspace`, matching `support/yang-enc/yang-schema.rb`'s
//! `PersistentYangSchema`, minus its on-disk parsed-schema cache --
//! re-parsing from scratch every invocation is the point), or a
//! checksum-addressed tarball downloaded from one of the two mirrors
//! mup1cc uses (matching `get_yang_schema`/`download_remote_catalog`).
//!
//! Either path lands on the same `CatalogFiles` (raw .yang/.sid source
//! text), fed straight into `schema::build` -- only the *download* of the
//! raw catalog files is ever cached, never the parsed schema.
//!
//! There's no HTTP/TLS library in this codebase at all: fetching the
//! tarball's bytes is always delegated to an external command --
//! `curl` by default (matching the real Ruby reference's own `wget`-
//! via-backtick approach, `support/scripts/mup1cc:98-103`), or whatever
//! `--catalog-fetcher` names instead, so it's entirely up to that
//! command (and its own TLS/certificate configuration) how network
//! security is handled. The contract is minimal and leaves no files
//! behind: given the checksum as its only argument, write the
//! catalog's `.tar.gz` bytes to stdout and exit 0 -- this module does
//! the gunzip/untar itself, in memory.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::schema::Schema;

pub struct CatalogFiles {
    pub yang: Vec<String>,
    pub sid: Vec<String>,
    pub ui_tailoring: Option<String>,
}

#[derive(Deserialize)]
struct UiTailoringFile {
    #[serde(rename = "ui-tailoring")]
    ui_tailoring: UiTailoringBody,
}

#[derive(Deserialize)]
struct UiTailoringBody {
    #[serde(rename = "identity-accept-list")]
    identity_accept_list: Vec<UiTailoringAcceptListEntry>,
    #[serde(rename = "identityref-accept-list", default)]
    identityref_accept_list: Vec<UiTailoringIdentityrefAcceptListEntry>,
    #[serde(rename = "yang-data-accept-list", default)]
    yang_data_accept_list: Vec<UiTailoringYangData>,
}

#[derive(Deserialize)]
struct UiTailoringAcceptListEntry {
    base: String,
    #[serde(rename = "accept-list")]
    accept_list: Vec<UiTailoringIdentity>,
}

#[derive(Deserialize)]
struct UiTailoringIdentityrefAcceptListEntry {
    leaf: String,
    #[serde(rename = "accept-list")]
    accept_list: Vec<UiTailoringIdentity>,
}

#[derive(Deserialize)]
struct UiTailoringIdentity {
    identity: String,
}

#[derive(Deserialize)]
struct UiTailoringYangData {
    name: String,
}

#[derive(Debug, Default, PartialEq)]
struct UiTailoring {
    identity_accept_lists: Vec<(String, Vec<String>)>,
    identityref_accept_lists: Vec<(String, Vec<String>)>,
    implemented_yang_data: Vec<String>,
}

fn accepted_names(accept_list: Vec<UiTailoringIdentity>) -> Vec<String> {
    accept_list.into_iter().map(|i| i.identity).collect()
}

fn as_slices<'a>(
    lists: &'a [(String, Vec<String>)],
    slices: &'a [Vec<&'a str>],
) -> Vec<(&'a str, &'a [&'a str])> {
    lists
        .iter()
        .zip(slices)
        .map(|((key, _), accepted)| (key.as_str(), accepted.as_slice()))
        .collect()
}

fn accepted_slices(lists: &[(String, Vec<String>)]) -> Vec<Vec<&str>> {
    lists
        .iter()
        .map(|(_, accepted)| accepted.iter().map(String::as_str).collect())
        .collect()
}

fn parse_ui_tailoring(text: &str) -> R<UiTailoring> {
    let file: UiTailoringFile =
        serde_yml::from_str(text).map_err(|e| err(format!("parsing ui-tailoring.yaml: {e}")))?;
    let identity_accept_lists = file
        .ui_tailoring
        .identity_accept_list
        .into_iter()
        .map(|entry| (entry.base, accepted_names(entry.accept_list)))
        .collect();
    let identityref_accept_lists = file
        .ui_tailoring
        .identityref_accept_list
        .into_iter()
        .map(|entry| (entry.leaf, accepted_names(entry.accept_list)))
        .collect();
    let implemented_yang_data = file
        .ui_tailoring
        .yang_data_accept_list
        .into_iter()
        .map(|entry| entry.name)
        .collect();
    Ok(UiTailoring {
        identity_accept_lists,
        identityref_accept_lists,
        implemented_yang_data,
    })
}

impl CatalogFiles {
    fn ui_tailoring(&self) -> R<UiTailoring> {
        match &self.ui_tailoring {
            Some(text) => parse_ui_tailoring(text),
            None => Ok(UiTailoring::default()),
        }
    }

    pub fn build_schema(&self) -> R<Schema> {
        let ui_tailoring = self.ui_tailoring()?;
        let by_base_slices = accepted_slices(&ui_tailoring.identity_accept_lists);
        let identity_accept_lists = as_slices(&ui_tailoring.identity_accept_lists, &by_base_slices);
        let by_leaf_slices = accepted_slices(&ui_tailoring.identityref_accept_lists);
        let identityref_accept_lists =
            as_slices(&ui_tailoring.identityref_accept_lists, &by_leaf_slices);
        let implemented_yang_data: Vec<&str> = ui_tailoring
            .implemented_yang_data
            .iter()
            .map(String::as_str)
            .collect();
        crate::schema::build(
            &self.yang,
            &self.sid,
            &implemented_yang_data,
            &identity_accept_lists,
            &identityref_accept_lists,
        )
        .map_err(|e| err(e.to_string()))
    }
}

#[derive(Debug)]
pub struct CatalogError(pub String);
impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for CatalogError {}
impl From<std::io::Error> for CatalogError {
    fn from(e: std::io::Error) -> Self {
        CatalogError(e.to_string())
    }
}

type R<T> = Result<T, CatalogError>;

fn err(msg: impl Into<String>) -> CatalogError {
    CatalogError(msg.into())
}

/// Remote catalog mirrors, tried in order (`mup1cc:91-95`).
pub const REMOTE_CATALOGS: &[&str] = &[
    "http://mscc-ent-open-source.s3-website-eu-west-1.amazonaws.com/public_root/velocitydrivesp/yang-by-sha",
    "https://artifacts.microchip.com/artifactory/UNGE-generic-local/lmstax/yang-by-sha",
];

/// Walk up from `start` looking for the repo root, identified the same
/// way as any checkout of this repo's own tooling would recognize
/// itself: the presence of `support/scripts/gen-cc-nodes.yaml`.
pub fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join("support/scripts/gen-cc-nodes.yaml").is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

#[derive(Deserialize)]
struct GenCcNodesCfg {
    #[serde(rename = "sid-files")]
    sid_files: Vec<String>,
}

/// Load the workspace's own YANG catalog: exactly the files listed in
/// `support/scripts/gen-cc-nodes.yaml`'s `sid-files`, resolved against
/// `docs/sw_refs/yang/` -- not a directory glob (that directory also
/// holds a `new-sid-files/` staging subdirectory and historical `.sid`
/// revisions that must be ignored).
pub fn load_workspace(repo_root: &Path) -> R<CatalogFiles> {
    let cfg_path = repo_root.join("support/scripts/gen-cc-nodes.yaml");
    let yang_dir = repo_root.join("docs/sw_refs/yang");

    let cfg_text = fs::read_to_string(&cfg_path)
        .map_err(|e| err(format!("reading {}: {e}", cfg_path.display())))?;
    let cfg: GenCcNodesCfg = serde_yml::from_str(&cfg_text)
        .map_err(|e| err(format!("parsing {}: {e}", cfg_path.display())))?;

    let mut yang = Vec::with_capacity(cfg.sid_files.len());
    let mut sid = Vec::with_capacity(cfg.sid_files.len());
    for entry in &cfg.sid_files {
        let sid_path = yang_dir.join(entry);
        let yang_name = format!("{}.yang", entry.split('@').next().unwrap_or(entry));
        let yang_path = yang_dir.join(&yang_name);
        sid.push(
            fs::read_to_string(&sid_path)
                .map_err(|e| err(format!("reading {}: {e}", sid_path.display())))?,
        );
        yang.push(
            fs::read_to_string(&yang_path)
                .map_err(|e| err(format!("reading {}: {e}", yang_path.display())))?,
        );
    }
    let ui_tailoring_path = repo_root.join("docs/sw_refs/yang/ui-tailoring.yaml");
    let ui_tailoring = fs::read_to_string(&ui_tailoring_path).ok();
    Ok(CatalogFiles {
        yang,
        sid,
        ui_tailoring,
    })
}

/// Load every `.yang`/`.sid` file directly in `dir` (a downloaded and
/// extracted catalog tarball's contents).
pub fn load_from_dir(dir: &Path) -> R<CatalogFiles> {
    let mut files = CatalogFiles {
        yang: Vec::new(),
        sid: Vec::new(),
        ui_tailoring: None,
    };
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            files.add(name, fs::read_to_string(&path)?);
        }
    }
    Ok(files)
}

pub fn load_from_tarball(bytes: &[u8]) -> R<CatalogFiles> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut files = CatalogFiles {
        yang: Vec::new(),
        sid: Vec::new(),
        ui_tailoring: None,
    };
    for entry in archive
        .entries()
        .map_err(|e| err(format!("reading the catalog tarball: {e}")))?
    {
        let mut entry = entry.map_err(|e| err(format!("reading the catalog tarball: {e}")))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| err(format!("reading the catalog tarball: {e}")))?
            .into_owned();
        if path.components().count() != 1 {
            continue;
        }
        let name = path.to_str().unwrap_or("").to_string();
        let mut content = String::new();
        std::io::Read::read_to_string(&mut entry, &mut content)
            .map_err(|e| err(format!("reading {name} from the catalog tarball: {e}")))?;
        files.add(&name, content);
    }
    Ok(files)
}

impl CatalogFiles {
    fn add(&mut self, file_name: &str, content: String) {
        if file_name == "ui-tailoring.yaml" {
            self.ui_tailoring = Some(content);
        } else if file_name.ends_with(".yang") {
            self.yang.push(content);
        } else if file_name.ends_with(".sid") {
            self.sid.push(content);
        }
    }
}

/// Fetch the catalog tarball named by `checksum` (the DUT's YANG-library
/// checksum, from SID 29304 / `ietf-constrained-yang-library:yang-
/// library/checksum`) and extract it into `dest_dir`. `fetcher`, if
/// given, names an external command to run instead of the built-in
/// `curl` default (see the module doc comment for the contract it must
/// satisfy). Only the download is skipped on a repeat run (`dest_dir`
/// already populated) -- the schema itself is always rebuilt fresh.
pub fn download_and_extract(
    checksum: &str,
    dest_dir: &Path,
    fetcher: Option<&str>,
    verbose: bool,
) -> R<()> {
    if dest_dir.join(".complete").is_file() {
        if verbose {
            eprintln!(
                "catalog for {checksum} already downloaded at {}",
                dest_dir.display()
            );
        }
        return Ok(());
    }
    fs::create_dir_all(dest_dir)?;

    let tarball = match fetcher {
        Some(command) => run_external_fetcher(command, checksum, verbose)?,
        None => fetch_with_curl(checksum, verbose)?,
    };
    extract_tarball(&tarball, dest_dir)?;

    fs::write(dest_dir.join(".complete"), b"")?;
    Ok(())
}

fn extract_tarball(bytes: &[u8], dest_dir: &Path) -> R<()> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    archive
        .unpack(dest_dir)
        .map_err(|e| err(format!("extracting the catalog tarball: {e}")))
}

/// The built-in default: try each known mirror in turn with a plain
/// `curl -fsSL <url>`, capturing its stdout as the tarball's bytes.
fn fetch_with_curl(checksum: &str, verbose: bool) -> R<Vec<u8>> {
    let mut last_err = None;
    for mirror in REMOTE_CATALOGS {
        let url = format!("{mirror}/{checksum}.tar.gz");
        if verbose {
            eprintln!("trying {url}...");
        }
        match run_command_capturing_stdout("curl", &["-fsSL", &url]) {
            Ok(bytes) => {
                if verbose {
                    eprintln!("catalog found in\n  {mirror}");
                }
                return Ok(bytes);
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(err(format!(
        "couldn't download the YANG catalog for checksum {checksum} from any known mirror -- check your network connection, or this device's firmware may be too new or too old for this tool. Last error: {}",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    )))
}

/// Run a `--catalog-fetcher` replacement: called as `<command>
/// <checksum>`, must write the catalog's `.tar.gz` bytes to stdout and
/// exit 0.
fn run_external_fetcher(command: &str, checksum: &str, verbose: bool) -> R<Vec<u8>> {
    if verbose {
        eprintln!("running catalog fetcher: {command} {checksum}");
    }
    run_command_capturing_stdout(command, &[checksum])
}

fn run_command_capturing_stdout(program: &str, args: &[&str]) -> R<Vec<u8>> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                err(format!(
                    "the '{program}' command isn't available on this system"
                ))
            } else {
                err(format!("couldn't run '{program}': {e}"))
            }
        })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(err(format!(
            "'{program}' exited with {} -- {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn parse_ui_tailoring_round_trips_the_real_interface_type_and_coreconf_error_entries() {
        let text = r#"
ui-tailoring:
  identity-accept-list:
  - base: "ietf-interfaces:interface-type"
    accept-list:
    - identity: "iana-if-type:ethernetCsmacd"
    - identity: "iana-if-type:ieee8023adLag"
    - identity: "iana-if-type:l3ipvlan"

  yang-data-accept-list:
    - name: "ietf-coreconf:coreconf-error"
"#;
        let tailoring = parse_ui_tailoring(text).unwrap();
        assert_eq!(
            tailoring,
            UiTailoring {
                identity_accept_lists: vec![(
                    "ietf-interfaces:interface-type".to_string(),
                    vec![
                        "iana-if-type:ethernetCsmacd".to_string(),
                        "iana-if-type:ieee8023adLag".to_string(),
                        "iana-if-type:l3ipvlan".to_string(),
                    ]
                )],
                identityref_accept_lists: Vec::new(),
                implemented_yang_data: vec!["ietf-coreconf:coreconf-error".to_string()],
            }
        );
    }

    #[test]
    fn parse_ui_tailoring_without_a_yang_data_accept_list_implements_no_yang_data() {
        let text = r#"
ui-tailoring:
  identity-accept-list: []
"#;
        assert_eq!(parse_ui_tailoring(text).unwrap(), UiTailoring::default());
    }

    #[test]
    fn parse_ui_tailoring_reads_the_identityref_accept_list() {
        let text = r#"
ui-tailoring:
  identity-accept-list: []
  identityref-accept-list:
  - leaf: "/ietf-routing:routing/ribs/rib/address-family"
    accept-list:
    - identity: "ietf-routing:ipv4"
"#;
        assert_eq!(
            parse_ui_tailoring(text).unwrap().identityref_accept_lists,
            vec![(
                "/ietf-routing:routing/ribs/rib/address-family".to_string(),
                vec!["ietf-routing:ipv4".to_string()]
            )]
        );
    }

    #[test]
    fn ui_tailoring_is_empty_when_no_ui_tailoring_file_is_present() {
        let files = CatalogFiles {
            yang: Vec::new(),
            sid: Vec::new(),
            ui_tailoring: None,
        };
        assert_eq!(files.ui_tailoring().unwrap(), UiTailoring::default());
    }

    #[test]
    fn nonexistent_fetcher_command_gives_a_clear_error() {
        let err =
            run_command_capturing_stdout("this-program-does-not-exist-anywhere", &[]).unwrap_err();
        assert!(err.to_string().contains("isn't available"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn command_capturing_stdout_returns_its_stdout_bytes() {
        let bytes = run_command_capturing_stdout("printf", &["%s", "hello"]).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[cfg(unix)]
    #[test]
    fn a_failing_command_reports_its_stderr() {
        let err =
            run_command_capturing_stdout("sh", &["-c", "echo it broke >&2; exit 3"]).unwrap_err();
        assert!(err.to_string().contains("it broke"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn external_fetcher_contract_end_to_end() {
        // A fake `--catalog-fetcher`: ignores the checksum it's given
        // and just cats the bundled real catalog tarball to stdout --
        // exactly the contract a real replacement is expected to
        // satisfy, and exactly what the built-in `curl` default's own
        // stdout capture must handle too.
        let tarball = test_data_dir().join("e6311dd5be50af0f0286fd3a6fb218a1.tar.gz");
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
}
