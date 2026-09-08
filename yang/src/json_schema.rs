// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! JSON Schema (draft-07) generation from a built [`Schema`], ported from
//! `support/yang-enc/yang-enc.rb`'s `to_json_schema`/`type2schema`. This
//! is the schema the Ruby reference feeds to `json_schemer` to validate a
//! request/response before encoding/decoding -- generating it is also
//! this port's `fercc schema` subcommand (`yang-enc schema`'s equivalent).
//!
//! Checked byte-for-byte (modulo two narrow, allow-listed gaps) against
//! a real `yang-enc schema` run on the same catalog --
//! `yang/tests/json_schema_conformance.rs`. Three deliberate divergences
//! remain, all independent of each other:
//! - `properties/ietf-coreconf:error` (an `rc:yang-data` root, not a
//!   real datastore node) is entirely missing from this port's output:
//!   [`crate::schema`]'s flattened tree deliberately keeps `rc:yang-
//!   data` roots out of the root node's children (so a whole-tree GET/
//!   PUT doesn't enumerate them as if they were real paths), so
//!   `to_json_schema` never sees them as children of `schema.root`
//!   either. Allow-listed in the conformance test above.
//! - Two `maxLength` values (both a `binary` leaf's *default*,
//!   unrestricted length) render as `i64::MAX` here versus Ruby's exact
//!   `24595658764946068820` -- see `num`'s doc comment. Also allow-
//!   listed there; not otherwise worth chasing, since both values just
//!   mean "no real bound".
//! - `anydata board:factory_default_config` is schematized as `{}` (any
//!   value) rather than Ruby's recursive re-derivation of the *default*
//!   schema under content-format `put` -- replicating that would need a
//!   second, independently-acquired schema threaded through the whole
//!   call chain for the sake of one specific node. Not exercised by the
//!   conformance test's `content_format` (`"yang"`, where this
//!   distinction doesn't arise), so not allow-listed there; would need
//!   its own fixture (`content_format` `"put"`) to catch a regression.
//!
//! An `identityref`'s `enum` list is also sorted here for deterministic
//! output, unlike Ruby's identity-registration-order-derived one -- not
//! a divergence the conformance test can even see (it compares `enum`
//! arrays as sets), since `enum` is one either way, and any draft-07
//! validator treats it as one.

use serde_json::{Map, Value as Json};

use crate::codec::all_identity_bases;
use crate::schema::{Builtin, NodeId, Schema, TypeId};

const DATA_NODES: &[&str] = &["container", "leaf", "leaf-list", "list", "anydata", "anyxml"];

fn dedup_preserve_order(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items.into_iter().filter(|s| seen.insert(s.clone())).collect()
}

fn json_strings(items: impl IntoIterator<Item = String>) -> Json {
    Json::Array(items.into_iter().map(Json::String).collect())
}

/// A range/length bound as a JSON number. Every bound this port
/// produces fits in `i64` or, for the *default*, unrestricted string/
/// binary length upper bound (`u64::MAX`, per `default_ranges`), `u64`
/// exactly -- so both are tried before falling back to a clamp.
///
/// The clamp *does* trigger, in exactly one further-derived case: a
/// `binary` leaf's default (unrestricted) `maxLength` is the base64
/// char-count of the default `u64::MAX` byte length --
/// `4 * ((u64::MAX + 2) / 3)` = 24595658764946068820 -- which overflows
/// `u64` too. Ruby's arbitrary-precision arithmetic renders that exact
/// value; `serde_json::Number` can't without the `arbitrary_precision`
/// feature, so this clamps to `i64::MAX` instead. A real, if narrow,
/// fidelity gap versus the reference tool's output -- but not a
/// meaningful one: both values just mean "no real bound", for a
/// property no catalog in this codebase actually restricts. See
/// `yang/tests/json_schema_conformance.rs` for where this is allow-
/// listed against a real `yang-enc schema` diff.
fn num(v: i128) -> Json {
    if let Ok(i) = i64::try_from(v) {
        Json::Number(i.into())
    } else if let Ok(u) = u64::try_from(v) {
        Json::Number(u.into())
    } else {
        Json::Number(if v > 0 { i64::MAX.into() } else { i64::MIN.into() })
    }
}

/// Mirrors `to_json_schema` (yang-enc.rb:1007-1129). `content_format` is
/// one of `"yang"`/`"fetch"`/`"ipatch"`/`"get"`/`"put"`/`"post"`, same as
/// the codec's own content-format strings.
pub fn to_json_schema(schema: &Schema, node: NodeId, content_format: &str) -> Json {
    let n = schema.node(node);

    let mut properties = Map::new();
    let mut required = Vec::new();
    for &child_id in &n.children {
        let child = schema.node(child_id);
        if matches!(content_format, "ipatch" | "put") && !child.config {
            continue;
        }
        if DATA_NODES.contains(&child.kw.as_str()) {
            properties.insert(child.name.clone(), to_json_schema(schema, child_id, content_format));
        }
        if child.kw == "input" || child.kw == "output" {
            properties.insert(child.kw.clone(), to_json_schema(schema, child_id, content_format));
        }
        if child.mandatory {
            required.push(child.name.clone());
        }
    }

    let mut result = match n.kw.as_str() {
        "module" => {
            let mut m = Map::new();
            m.insert("title".into(), Json::String(n.name.clone()));
            m.insert("$schema".into(), Json::String("http://json-schema.org/draft-07/schema#".into()));
            m.insert("type".into(), Json::String("object".into()));
            m.insert("additionalProperties".into(), Json::Bool(false));
            m.insert("properties".into(), Json::Object(properties));
            if !required.is_empty() {
                m.insert("required".into(), json_strings(required));
            }
            Json::Object(m)
        }

        "container" => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("object".into()));
            m.insert("additionalProperties".into(), Json::Bool(false));
            m.insert("properties".into(), Json::Object(properties));
            if !required.is_empty() {
                m.insert("required".into(), json_strings(required));
            }
            Json::Object(m)
        }

        "leaf" => type2schema(schema, n.type_id.expect("leaf without a type")),

        "leaf-list" => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("array".into()));
            m.insert("items".into(), type2schema(schema, n.type_id.expect("leaf-list without a type")));
            if n.config {
                m.insert("uniqueItems".into(), Json::Bool(true));
            }
            Json::Object(m)
        }

        "list" => {
            let mut keys_and_required = n.keys.clone();
            keys_and_required.extend(required);
            let keys_and_required = dedup_preserve_order(keys_and_required);

            let mut item = Map::new();
            item.insert("type".into(), Json::String("object".into()));
            item.insert("additionalProperties".into(), Json::Bool(false));
            item.insert("required".into(), json_strings(keys_and_required.clone()));
            item.insert("properties".into(), Json::Object(properties.clone()));

            let mut list_schema = Map::new();
            list_schema.insert("type".into(), Json::String("array".into()));
            list_schema.insert("items".into(), Json::Object(item));

            if matches!(content_format, "fetch" | "ipatch") {
                // Accept either array or object in FETCH and iPATCH.
                let mut bare = Map::new();
                bare.insert("type".into(), Json::String("object".into()));
                bare.insert("additionalProperties".into(), Json::Bool(false));
                bare.insert("required".into(), json_strings(keys_and_required));
                bare.insert("properties".into(), Json::Object(properties));
                let mut one_of = Map::new();
                one_of.insert("oneOf".into(), Json::Array(vec![Json::Object(list_schema), Json::Object(bare)]));
                Json::Object(one_of)
            } else {
                Json::Object(list_schema)
            }
        }

        "anydata" => Json::Object(Map::new()),

        "anyxml" => Json::Object(Map::new()),

        "input" | "output" => {
            if n.children.is_empty() {
                let mut m = Map::new();
                m.insert("type".into(), Json::String("null".into()));
                Json::Object(m)
            } else {
                let mut m = Map::new();
                m.insert("type".into(), Json::String("object".into()));
                m.insert("additionalProperties".into(), Json::Bool(false));
                m.insert("properties".into(), Json::Object(properties));
                // Ruby recomputes `required` here without the config-skip
                // filter the prologue above applies for ipatch/put
                // (yang-enc.rb:1095-1096) -- an inconsistency in the
                // reference, replicated rather than "fixed", per this
                // port's own fidelity goal.
                let required: Vec<String> = n.children.iter().map(|&c| schema.node(c)).filter(|c| c.mandatory).map(|c| c.name.clone()).collect();
                if !required.is_empty() {
                    m.insert("required".into(), json_strings(required));
                }
                Json::Object(m)
            }
        }

        "action" | "rpc" => {
            let input_node = n.children.iter().copied().find(|&c| schema.node(c).kw == "input").expect("action/rpc always has a synthesized input node");
            let output_node = n.children.iter().copied().find(|&c| schema.node(c).kw == "output").expect("action/rpc always has a synthesized output node");
            if schema.node(input_node).children.is_empty() && schema.node(output_node).children.is_empty() {
                let mut m = Map::new();
                m.insert("type".into(), Json::String("null".into()));
                Json::Object(m)
            } else {
                let mut obj = Map::new();
                obj.insert("type".into(), Json::String("object".into()));
                obj.insert("additionalProperties".into(), Json::Bool(false));
                obj.insert("properties".into(), Json::Object(properties));
                let mut null = Map::new();
                null.insert("type".into(), Json::String("null".into()));
                let mut one_of = Map::new();
                one_of.insert("oneOf".into(), Json::Array(vec![Json::Object(null), Json::Object(obj)]));
                Json::Object(one_of)
            }
        }

        // Every other keyword (choice/case) is elided by the schema
        // builder's own flattening pass (crate::schema::Builder::finish)
        // and never reaches here as a child at all.
        other => panic!("to_json_schema: unexpected node keyword {other:?}"),
    };

    // Mirrors the `description` line after `to_json_schema`'s own `case`
    // (yang-enc.rb:1127): applied uniformly on top of whatever shape the
    // match above produced -- including a `leaf`'s `type2schema` result,
    // an empty `anydata`/`anyxml` `{}`, or a `oneOf`-wrapped list/rpc.
    if let (Some(desc), Json::Object(obj)) = (&n.description, &mut result) {
        obj.insert("description".to_string(), Json::String(desc.clone()));
    }
    result
}

/// Mirrors `type2schema` (yang-enc.rb:1131-1232).
fn type2schema(schema: &Schema, type_id: TypeId) -> Json {
    let t = schema.ty(type_id);
    match t.builtin {
        Builtin::Int8 | Builtin::Int16 | Builtin::Int32 | Builtin::Uint8 | Builtin::Uint16 | Builtin::Uint32 => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("integer".into()));
            m.insert(
                "anyOf".into(),
                Json::Array(
                    t.ranges
                        .iter()
                        .map(|&(min, max)| {
                            let mut r = Map::new();
                            r.insert("minimum".into(), num(min));
                            r.insert("maximum".into(), num(max));
                            Json::Object(r)
                        })
                        .collect(),
                ),
            );
            Json::Object(m)
        }

        // See RFC 7951 section 6.1.
        Builtin::Int64 | Builtin::Uint64 => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("string".into()));
            Json::Object(m)
        }

        // See RFC 7951 section 6.1.
        Builtin::Decimal64 => {
            let fd = t.fraction_digits.unwrap_or(0);
            let mut m = Map::new();
            m.insert("type".into(), Json::String("string".into()));
            m.insert("pattern".into(), Json::String(format!("^(\\+|-)?\\d*(\\.\\d{{0,{fd}}})?$")));
            Json::Object(m)
        }

        Builtin::Boolean => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("boolean".into()));
            Json::Object(m)
        }

        Builtin::Union => {
            let mut m = Map::new();
            m.insert("anyOf".into(), Json::Array(t.union_members.iter().map(|&member| type2schema(schema, member)).collect()));
            Json::Object(m)
        }

        Builtin::Enumeration => {
            let mut m = Map::new();
            m.insert("enum".into(), json_strings(t.enums.iter().map(|e| e.name.clone())));
            Json::Object(m)
        }

        // See RFC 7951 section 6.9.
        Builtin::Empty => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("array".into()));
            let mut items = Map::new();
            items.insert("type".into(), Json::String("null".into()));
            m.insert("items".into(), Json::Object(items));
            m.insert("minItems".into(), Json::Number(1.into()));
            m.insert("maxItems".into(), Json::Number(1.into()));
            Json::Object(m)
        }

        // See RFC 7951 section 6.6.
        Builtin::Binary => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("string".into()));
            m.insert("contentEncoding".into(), Json::String("base64".into()));
            m.insert(
                "anyOf".into(),
                Json::Array(
                    t.ranges
                        .iter()
                        .map(|&(min, max)| {
                            // n bytes encode to 4 * ceil(n/3) base64 chars.
                            let mut r = Map::new();
                            r.insert("minLength".into(), num(4 * ((min + 2) / 3)));
                            r.insert("maxLength".into(), num(4 * ((max + 2) / 3)));
                            Json::Object(r)
                        })
                        .collect(),
                ),
            );
            Json::Object(m)
        }

        // See RFC 7951 section 6.5.
        Builtin::Bits => {
            let words = format!("({})", t.bits.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join("|"));
            let mut m = Map::new();
            m.insert("type".into(), Json::String("string".into()));
            m.insert("pattern".into(), Json::String(format!("^{words}?(\\s{words})*$")));
            Json::Object(m)
        }

        Builtin::Leafref => {
            let target = t.leafref_target.expect("leafref with an unresolved target");
            let target_type = schema.node(target).type_id.expect("leafref target without a type");
            type2schema(schema, target_type)
        }

        // The permissible identity names are the intersection of
        // identities derived from each base identity. See RFC 7950
        // section 9.10.2 -- note "or equal to" there: this port's own
        // `all_identity_bases`/`Schema::derived_from` (used for encode/
        // decode, where a value equal to the base itself really is
        // legal per the RFC) includes each base itself in the returned
        // set, but Ruby's `Identity#derived_from` this mirrors for JSON-
        // Schema generation specifically does not ("@derived +
        // @derived.flat_map(&:derived_from)" -- an identity's *own*
        // list of things derived from it, never including itself). So
        // the bases themselves are excluded here, deliberately
        // diverging from `all_identity_bases`'s own RFC-correct
        // semantics, to match what the real reference tool actually
        // emits (confirmed by diffing this port's `fercc schema` output
        // against a real `yang-enc schema` run on the same catalog --
        // see `yang/tests/json_schema_conformance.rs`).
        Builtin::Identityref => {
            let candidates = all_identity_bases(schema, t).unwrap_or_default();
            let source_module = t.source_module.as_deref();
            let mut enums = Vec::new();
            for id in candidates {
                if t.identity_bases.contains(&id) {
                    continue;
                }
                let identity = schema.identity(id);
                enums.push(format!("{}:{}", identity.module, identity.name));
                if Some(identity.module.as_str()) == source_module {
                    enums.push(identity.name.clone());
                }
            }
            enums.sort();
            let mut m = Map::new();
            m.insert("enum".into(), json_strings(enums));
            Json::Object(m)
        }

        Builtin::String => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("string".into()));
            m.insert(
                "anyOf".into(),
                Json::Array(
                    t.ranges
                        .iter()
                        .map(|&(min, max)| {
                            let mut r = Map::new();
                            r.insert("minLength".into(), num(min));
                            r.insert("maxLength".into(), num(max));
                            Json::Object(r)
                        })
                        .collect(),
                ),
            );
            if !t.patterns.is_empty() {
                m.insert(
                    "allOf".into(),
                    Json::Array(
                        t.patterns
                            .iter()
                            .map(|p| {
                                let mut r = Map::new();
                                r.insert("pattern".into(), Json::String(p.clone()));
                                Json::Object(r)
                            })
                            .collect(),
                    ),
                );
            }
            Json::Object(m)
        }

        // Ruby's `type2schema` else-branch (yang-enc.rb:1228-1229):
        // anything not matched above -- in practice just
        // `instance-identifier`, since every other RFC 7950 builtin has
        // its own case.
        Builtin::InstanceIdentifier => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("string".into()));
            Json::Object(m)
        }
    }
}
