//! Conformance tests against the ~387 real YAML/CBOR fixture pairs copied
//! from sw-velocitydrive-devclient's test-data/ (see
//! mup1cc-rs/rust-mup1cc.txt and the top-level commit history for
//! provenance), plus the bundled YANG catalog tarball they were generated
//! against. Each case is checked both directions: encoding the YAML
//! fixture must produce the reference .cbor bytes exactly, and decoding
//! the reference .cbor then re-encoding it must round-trip byte-identically.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value as Json;
use yang::codec::{self, ContentFormat};
use yang::schema::Schema;

fn test_data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("test-data")
}

fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
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
    })
}

fn content_format_for(name: &str) -> Option<ContentFormat> {
    if name.ends_with("-ipatch-req") {
        Some(ContentFormat::Ipatch)
    } else if name.ends_with("-fetch-req") || name.ends_with("-fetch-res") {
        Some(ContentFormat::Fetch)
    } else if name.ends_with("-post-req") || name.ends_with("-post-res") {
        Some(ContentFormat::Post)
    } else {
        None
    }
}

#[test]
fn fixture_corpus_round_trips() {
    let schema = schema();
    let dir = test_data_dir();

    let mut entries: Vec<_> = fs::read_dir(&dir).expect("read test-data dir").filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.path());

    let mut cases = 0;
    let mut failures: HashMap<String, String> = HashMap::new();

    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }
        let name = path.file_stem().unwrap().to_str().unwrap().to_string();
        let Some(cf) = content_format_for(&name) else { continue };
        let cbor_path = dir.join(format!("{name}.cbor"));
        if !cbor_path.exists() {
            continue;
        }
        cases += 1;

        let outcome = (|| -> Result<(), String> {
            let yaml_text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
            let expected_cbor = fs::read(&cbor_path).map_err(|e| e.to_string())?;

            let value: Json = serde_yaml_ng::from_str(&yaml_text).map_err(|e| format!("yaml parse: {e}"))?;
            let items = value.as_array().ok_or("top-level yaml is not a sequence")?;
            let got_cbor = codec::json_seq_to_cbor(schema, items, cf).map_err(|e| format!("encode: {e}"))?;
            if got_cbor != expected_cbor {
                return Err("encode mismatch".to_string());
            }

            let decoded = codec::cbor_seq_to_json(schema, &expected_cbor, cf).map_err(|e| format!("decode: {e}"))?;
            let re_encoded = codec::json_seq_to_cbor(schema, &decoded, cf).map_err(|e| format!("re-encode: {e}"))?;
            if re_encoded != expected_cbor {
                return Err("roundtrip mismatch".to_string());
            }
            Ok(())
        })();

        if let Err(e) = outcome {
            failures.insert(name, e);
        }
    }

    assert!(cases > 300, "expected >300 fixture cases discovered, found {cases} -- test-data/ missing?");
    assert!(failures.is_empty(), "{} unexpected fixture failures:\n{failures:#?}", failures.len());
}
