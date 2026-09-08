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

pub struct CatalogFiles {
    pub yang: Vec<String>,
    pub sid: Vec<String>,
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

    let cfg_text = fs::read_to_string(&cfg_path).map_err(|e| err(format!("reading {}: {e}", cfg_path.display())))?;
    let cfg: GenCcNodesCfg = serde_yaml_ng::from_str(&cfg_text).map_err(|e| err(format!("parsing {}: {e}", cfg_path.display())))?;

    let mut yang = Vec::with_capacity(cfg.sid_files.len());
    let mut sid = Vec::with_capacity(cfg.sid_files.len());
    for entry in &cfg.sid_files {
        let sid_path = yang_dir.join(entry);
        let yang_name = format!("{}.yang", entry.split('@').next().unwrap_or(entry));
        let yang_path = yang_dir.join(&yang_name);
        sid.push(fs::read_to_string(&sid_path).map_err(|e| err(format!("reading {}: {e}", sid_path.display())))?);
        yang.push(fs::read_to_string(&yang_path).map_err(|e| err(format!("reading {}: {e}", yang_path.display())))?);
    }
    Ok(CatalogFiles { yang, sid })
}

/// Load every `.yang`/`.sid` file directly in `dir` (a downloaded and
/// extracted catalog tarball's contents).
pub fn load_from_dir(dir: &Path) -> R<CatalogFiles> {
    let mut yang = Vec::new();
    let mut sid = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        match path.extension().and_then(|e| e.to_str()) {
            Some("yang") => yang.push(fs::read_to_string(&path)?),
            Some("sid") => sid.push(fs::read_to_string(&path)?),
            _ => {}
        }
    }
    Ok(CatalogFiles { yang, sid })
}

/// Fetch the catalog tarball named by `checksum` (the DUT's YANG-library
/// checksum, from SID 29304 / `ietf-constrained-yang-library:yang-
/// library/checksum`) and extract it into `dest_dir`. `fetcher`, if
/// given, names an external command to run instead of the built-in
/// `curl` default (see the module doc comment for the contract it must
/// satisfy). Only the download is skipped on a repeat run (`dest_dir`
/// already populated) -- the schema itself is always rebuilt fresh.
pub fn download_and_extract(checksum: &str, dest_dir: &Path, fetcher: Option<&str>, verbose: bool) -> R<()> {
    if dest_dir.join(".complete").is_file() {
        if verbose {
            eprintln!("catalog for {checksum} already downloaded at {}", dest_dir.display());
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
    archive.unpack(dest_dir).map_err(|e| err(format!("extracting the catalog tarball: {e}")))
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
    let output = std::process::Command::new(program).args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            err(format!("the '{program}' command isn't available on this system"))
        } else {
            err(format!("couldn't run '{program}': {e}"))
        }
    })?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(err(format!("'{program}' exited with {} -- {}", output.status, String::from_utf8_lossy(&output.stderr).trim())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn test_data_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("test-data")
    }

    #[cfg(unix)]
    fn unique_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("rcc-catalog-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn nonexistent_fetcher_command_gives_a_clear_error() {
        let err = run_command_capturing_stdout("this-program-does-not-exist-anywhere", &[]).unwrap_err();
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
        let err = run_command_capturing_stdout("sh", &["-c", "echo it broke >&2; exit 3"]).unwrap_err();
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
        fs::write(&script_path, format!("#!/bin/sh\nexec cat '{}'\n", tarball.display())).unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        let dest_dir = unique_path("dest");
        download_and_extract("ignored-checksum", &dest_dir, Some(script_path.to_str().unwrap()), false).unwrap();

        let files = load_from_dir(&dest_dir).unwrap();
        assert!(!files.yang.is_empty());
        assert!(!files.sid.is_empty());
        assert!(dest_dir.join(".complete").is_file());

        // A repeat call must be short-circuited by the `.complete`
        // sentinel before ever invoking the fetcher again -- point at
        // a command that would fail loudly if it were actually run.
        download_and_extract("ignored-checksum", &dest_dir, Some("this-program-does-not-exist-anywhere"), false).unwrap();

        let _ = fs::remove_dir_all(&dest_dir);
        let _ = fs::remove_file(&script_path);
    }
}
