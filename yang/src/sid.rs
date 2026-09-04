//! `.sid` file parsing (RFC 9595 JSON format).
//!
//! ```json
//! {
//!   "module-name": "board",
//!   "module-revision": "2026-03-31",
//!   "items": [
//!     {"namespace": "module", "identifier": "board", "sid": 38500},
//!     {"namespace": "data", "identifier": "/board:capabilities", "sid": 38501}
//!   ]
//! }
//! ```

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Namespace {
    Module,
    Data,
    Identity,
    /// `feature` SIDs exist (RFC 9595) but are irrelevant to the SID-CBOR
    /// wire codec -- `if-feature` isn't evaluated (see `schema` module
    /// docs), so these are parsed and otherwise ignored.
    Feature,
}

#[derive(Debug, Clone)]
pub struct SidItem {
    pub namespace: Namespace,
    /// For `Module`/`Identity`: a bare name. For `Data`: a schema-node-id
    /// like `/board:capabilities/arp_count`.
    pub identifier: String,
    pub sid: i64,
}

#[derive(Debug, Clone)]
pub struct SidFile {
    pub module_name: String,
    pub items: Vec<SidItem>,
}

#[derive(Deserialize)]
struct RawSidFile {
    #[serde(rename = "module-name")]
    module_name: String,
    items: Vec<RawItem>,
}

#[derive(Deserialize)]
struct RawItem {
    namespace: String,
    identifier: String,
    sid: i64,
}

pub fn parse(src: &str) -> Result<SidFile, String> {
    let raw: RawSidFile = serde_json::from_str(src).map_err(|e| e.to_string())?;
    let mut items = Vec::with_capacity(raw.items.len());
    for it in raw.items {
        let namespace = match it.namespace.as_str() {
            "module" => Namespace::Module,
            "data" => Namespace::Data,
            "identity" => Namespace::Identity,
            "feature" => Namespace::Feature,
            other => return Err(format!("unknown SID namespace {other:?}")),
        };
        items.push(SidItem { namespace, identifier: it.identifier, sid: it.sid });
    }
    Ok(SidFile { module_name: raw.module_name, items })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_sid_file() {
        let src = r#"{
            "module-name": "board",
            "module-revision": "2026-03-31",
            "items": [
                {"namespace": "module", "identifier": "board", "status": "unstable", "sid": 38500},
                {"namespace": "data", "identifier": "/board:capabilities", "status": "unstable", "sid": 38501}
            ]
        }"#;
        let f = parse(src).unwrap();
        assert_eq!(f.module_name, "board");
        assert_eq!(f.items.len(), 2);
        assert_eq!(f.items[0].namespace, Namespace::Module);
        assert_eq!(f.items[1].identifier, "/board:capabilities");
        assert_eq!(f.items[1].sid, 38501);
    }
}
