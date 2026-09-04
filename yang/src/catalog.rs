//! YANG catalog acquisition: either this repo's checked-out sources
//! (`--workspace`, matching `support/yang-enc/yang-schema.rb`'s
//! `PersistentYangSchema`, minus its on-disk parsed-schema cache -- see
//! `mup1cc-rs/rust-mup1cc.txt`: re-parsing from scratch every invocation
//! is the point), or a checksum-addressed tarball downloaded from one of
//! the two mirrors mup1cc.rb uses (matching `get_yang_schema`/
//! `download_remote_catalog`).
//!
//! Either path lands on the same `CatalogFiles` (raw .yang/.sid source
//! text), fed straight into `schema::build` -- only the *download* of the
//! raw catalog files is ever cached, never the parsed schema.

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

/// Remote catalog mirrors, tried in order (`support/scripts/mup1cc:91-95`).
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
/// library/checksum`) from the first mirror that has it, extracting into
/// `dest_dir`. Only the download is skipped on a repeat run (`dest_dir`
/// already populated) -- the schema itself is always rebuilt fresh.
pub fn download_and_extract(checksum: &str, dest_dir: &Path, verbose: bool) -> R<()> {
    if dest_dir.join(".complete").is_file() {
        if verbose {
            eprintln!("catalog for {checksum} already downloaded at {}", dest_dir.display());
        }
        return Ok(());
    }
    fs::create_dir_all(dest_dir)?;

    let mut last_err = None;
    for mirror in REMOTE_CATALOGS {
        let url = format!("{mirror}/{checksum}.tar.gz");
        if verbose {
            eprintln!("trying {url}...");
        }
        match fetch_and_extract(&url, dest_dir) {
            Ok(()) => {
                fs::write(dest_dir.join(".complete"), b"")?;
                if verbose {
                    eprintln!("catalog found in\n  {mirror}");
                }
                return Ok(());
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(err(format!(
        "remote catalog based on {checksum} not found! last error: {}",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    )))
}

fn fetch_and_extract(url: &str, dest_dir: &Path) -> R<()> {
    let mut response = ureq::get(url).call().map_err(|e| err(format!("GET {url}: {e}")))?;
    let gz = response.body_mut().as_reader();
    let tar = flate2::read::GzDecoder::new(gz);
    let mut archive = tar::Archive::new(tar);
    archive.unpack(dest_dir).map_err(|e| err(format!("extracting {url}: {e}")))?;
    Ok(())
}
