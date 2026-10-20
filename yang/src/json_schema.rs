// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! JSON Schema (draft-07) generation from a built [`crate::schema::Schema`], ported from
//! `support/yang-enc/yang-enc.rb`'s `to_json_schema`/`type2schema`. This
//! is the schema the Ruby reference feeds to `json_schemer` to validate a
//! request/response before encoding/decoding -- generating it is also
//! this port's `fercc schema` subcommand (`yang-enc schema`'s equivalent).
//!
//! Deliberate choices, all independent of each other:
//! - `rc:yang-data` roots (e.g. `ietf-coreconf:error`) aren't datastore
//!   nodes: [`crate::schema`] keeps them out of every module's
//!   `root_data_nodes` (so a whole-tree GET/PUT doesn't enumerate them as
//!   if they were real paths), so `to_json_schema_for_catalog` never sees
//!   them.
//! - A length bound equal to the built-in type's own is left out.
//! - Every `anydata`, `board:factory_default_config` included, is
//!   schematized as `{}` (any value). Validating that one's content
//!   against the datastore schema under content-format `put` would need
//!   a second, independently-acquired schema threaded through the whole
//!   call chain for the sake of one specific node.
//! - An `identityref`'s `enum` list is sorted, so the output is
//!   deterministic; any draft-07 validator treats `enum` as a set either
//!   way.

use serde_json::{Map, Value as Json};

use crate::schema::{default_ranges, Builtin, NodeRef, Schema, TypeRef};

type R<T> = Result<T, String>;

fn dedup_preserve_order(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

fn json_strings(items: impl IntoIterator<Item = String>) -> Json {
    Json::Array(items.into_iter().map(Json::String).collect())
}

/// A range/length bound as a JSON number, or an error if it is outside
/// what one can carry here (`i64` to `u64`).
pub(crate) fn num(v: i128) -> R<Json> {
    if let Ok(i) = i64::try_from(v) {
        Ok(Json::Number(i.into()))
    } else if let Ok(u) = u64::try_from(v) {
        Ok(Json::Number(u.into()))
    } else {
        Err(format!("the bound {v} doesn't fit in a JSON number"))
    }
}

fn explicit_bounds(t: TypeRef<'_>) -> Vec<(Option<i128>, Option<i128>)> {
    let default = default_ranges(t.builtin);
    let (Some(&(default_min, _)), Some(&(_, default_max))) = (default.first(), default.last())
    else {
        return Vec::new();
    };
    let last = t.ranges.len().saturating_sub(1);
    let mut out = Vec::new();
    for (i, &(min, max)) in t.ranges.iter().enumerate() {
        let min = if i == 0 && min == default_min {
            None
        } else {
            Some(min)
        };
        let max = if i == last && max == default_max {
            None
        } else {
            Some(max)
        };
        if min.is_some() || max.is_some() {
            out.push((min, max));
        }
    }
    out
}

fn base64_chars(bytes: i128) -> i128 {
    4 * ((bytes + 2) / 3)
}

fn insert_bounds(
    m: &mut Map<String, Json>,
    bounds: Vec<(Option<i128>, Option<i128>)>,
    min_key: &str,
    max_key: &str,
) -> R<()> {
    if bounds.is_empty() {
        return Ok(());
    }
    let mut parts = Vec::new();
    for (min, max) in bounds {
        let mut r = Map::new();
        if let Some(min) = min {
            r.insert(min_key.into(), num(min)?);
        }
        if let Some(max) = max {
            r.insert(max_key.into(), num(max)?);
        }
        parts.push(Json::Object(r));
    }
    m.insert("anyOf".into(), Json::Array(parts));
    Ok(())
}

pub fn to_json_schema_for_catalog(schema: &Schema, content_format: &str) -> R<Json> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for module in schema.modules() {
        for child in module.root_data_nodes() {
            if matches!(child.kw.as_str(), "rpc" | "action") {
                continue;
            }
            if matches!(content_format, "ipatch" | "put") && !child.config {
                continue;
            }
            properties.insert(child.name.clone(), to_json_schema(child, content_format)?);
            if child.mandatory {
                required.push(child.name.clone());
            }
        }
    }
    let mut m = Map::new();
    m.insert("title".into(), Json::String("data-tree-schema".to_string()));
    m.insert(
        "$schema".into(),
        Json::String("http://json-schema.org/draft-07/schema#".into()),
    );
    m.insert("type".into(), Json::String("object".into()));
    m.insert("additionalProperties".into(), Json::Bool(false));
    m.insert("properties".into(), Json::Object(properties));
    if !required.is_empty() {
        m.insert("required".into(), json_strings(required));
    }
    Ok(Json::Object(m))
}

/// Mirrors `to_json_schema` (yang-enc.rb:1007-1129). `content_format` is
/// one of `"yang"`/`"fetch"`/`"ipatch"`/`"get"`/`"put"`/`"post"`, same as
/// the codec's own content-format strings.
pub fn to_json_schema(node: NodeRef<'_>, content_format: &str) -> R<Json> {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for child in node.data_children() {
        if matches!(child.kw.as_str(), "rpc" | "action") {
            continue;
        }
        if matches!(content_format, "ipatch" | "put") && !child.config {
            continue;
        }
        properties.insert(child.name.clone(), to_json_schema(child, content_format)?);
        if child.mandatory {
            required.push(child.name.clone());
        }
    }

    let mut result = match node.kw.as_str() {
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

        "leaf" => type2schema(node.ty().expect("leaf without a type"))
            .map_err(|e| format!("{}: {e}", node.path()))?,

        "leaf-list" => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("array".into()));
            m.insert(
                "items".into(),
                type2schema(node.ty().expect("leaf-list without a type"))
                    .map_err(|e| format!("{}: {e}", node.path()))?,
            );
            if node.config {
                m.insert("uniqueItems".into(), Json::Bool(true));
            }
            Json::Object(m)
        }

        "list" => {
            let mut keys_and_required: Vec<String> = node.keys().map(|k| k.name.clone()).collect();
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
                one_of.insert(
                    "oneOf".into(),
                    Json::Array(vec![Json::Object(list_schema), Json::Object(bare)]),
                );
                Json::Object(one_of)
            } else {
                Json::Object(list_schema)
            }
        }

        "anydata" => Json::Object(Map::new()),

        "anyxml" => Json::Object(Map::new()),

        "input" | "output" => {
            if node.data_children().next().is_none() {
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
                let required: Vec<String> = node
                    .data_children()
                    .filter(|c| c.mandatory)
                    .map(|c| c.name.clone())
                    .collect();
                if !required.is_empty() {
                    m.insert("required".into(), json_strings(required));
                }
                Json::Object(m)
            }
        }

        "action" | "rpc" => {
            let input_node = node
                .data_children()
                .find(|c| c.kw == "input")
                .expect("action/rpc always has a synthesized input node");
            let output_node = node
                .data_children()
                .find(|c| c.kw == "output")
                .expect("action/rpc always has a synthesized output node");
            if input_node.data_children().next().is_none()
                && output_node.data_children().next().is_none()
            {
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
                one_of.insert(
                    "oneOf".into(),
                    Json::Array(vec![Json::Object(null), Json::Object(obj)]),
                );
                Json::Object(one_of)
            }
        }

        other => panic!("to_json_schema: unexpected node keyword {other:?}"),
    };

    // Mirrors the `description` line after `to_json_schema`'s own `case`
    // (yang-enc.rb:1127): applied uniformly on top of whatever shape the
    // match above produced -- including a `leaf`'s `type2schema` result,
    // an empty `anydata`/`anyxml` `{}`, or a `oneOf`-wrapped list/rpc.
    if let (Some(desc), Json::Object(obj)) = (&node.description, &mut result) {
        obj.insert("description".to_string(), Json::String(desc.clone()));
    }
    Ok(result)
}

/// Mirrors `type2schema` (yang-enc.rb:1131-1232).
fn type2schema(t: TypeRef<'_>) -> R<Json> {
    Ok(match t.builtin {
        Builtin::Int8
        | Builtin::Int16
        | Builtin::Int32
        | Builtin::Uint8
        | Builtin::Uint16
        | Builtin::Uint32 => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("integer".into()));
            let bounds = t
                .ranges
                .iter()
                .map(|&(min, max)| (Some(min), Some(max)))
                .collect();
            insert_bounds(&mut m, bounds, "minimum", "maximum")?;
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
            m.insert(
                "pattern".into(),
                Json::String(format!("^(\\+|-)?\\d*(\\.\\d{{0,{fd}}})?$")),
            );
            Json::Object(m)
        }

        Builtin::Boolean => {
            let mut m = Map::new();
            m.insert("type".into(), Json::String("boolean".into()));
            Json::Object(m)
        }

        Builtin::Union => {
            let mut m = Map::new();
            m.insert(
                "anyOf".into(),
                Json::Array(t.union_members().map(type2schema).collect::<R<Vec<_>>>()?),
            );
            Json::Object(m)
        }

        Builtin::Enumeration => {
            let mut m = Map::new();
            m.insert(
                "enum".into(),
                json_strings(t.enums.iter().map(|e| e.name.clone())),
            );
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
            // n bytes encode to 4 * ceil(n/3) base64 chars.
            let chars = explicit_bounds(t)
                .into_iter()
                .map(|(min, max)| (min.map(base64_chars), max.map(base64_chars)))
                .collect();
            insert_bounds(&mut m, chars, "minLength", "maxLength")?;
            Json::Object(m)
        }

        // See RFC 7951 section 6.5.
        Builtin::Bits => {
            let words = format!(
                "({})",
                t.bits
                    .iter()
                    .map(|b| b.name.as_str())
                    .collect::<Vec<_>>()
                    .join("|")
            );
            let mut m = Map::new();
            m.insert("type".into(), Json::String("string".into()));
            m.insert(
                "pattern".into(),
                Json::String(format!("^{words}?(\\s{words})*$")),
            );
            Json::Object(m)
        }

        Builtin::Leafref => {
            let target_type = t
                .leafref_target()
                .expect("leafref with an unresolved target")
                .ty()
                .expect("leafref target without a type");
            type2schema(target_type)?
        }

        // The permissible identity names are the intersection of
        // identities derived from each base identity. See RFC 7950
        // section 9.10.2 and 7.18.2 (derivation is irreflexive, so a
        // base identity is never among its own derived set) --
        // `TypeRef::identity_values` already excludes each base
        // identity itself, matching the real reference tool's own
        // `Identity#derived_from` ("@derived +
        // @derived.flat_map(&:derived_from)"), narrowed by an identity
        // or identityref accept-list where one applies.
        Builtin::Identityref => {
            let candidates = t.identity_values().unwrap_or_default();
            let source_module = t.source_module.as_deref();
            let mut enums = Vec::new();
            for identity in candidates {
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
            insert_bounds(&mut m, explicit_bounds(t), "minLength", "maxLength")?;
            if !t.patterns.is_empty() {
                m.insert(
                    "allOf".into(),
                    Json::Array(
                        t.patterns
                            .iter()
                            .map(|p| {
                                let mut r = Map::new();
                                r.insert("pattern".into(), Json::String(p.regex.clone()));
                                if p.invert_match {
                                    let mut not = Map::new();
                                    not.insert("not".into(), Json::Object(r));
                                    Json::Object(not)
                                } else {
                                    Json::Object(r)
                                }
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_inverted_pattern_is_a_negated_json_schema_pattern() {
        let src = r#"
            module pat {
              namespace "urn:pat";
              prefix p;

              leaf name {
                type string {
                  pattern "[a-z]+";
                  pattern "x.*" {
                    modifier invert-match;
                  }
                }
              }
            }
        "#;
        let schema = crate::schema::build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
        let ty = schema
            .node_by_schema_path("/pat:name")
            .unwrap()
            .ty()
            .unwrap();
        assert_eq!(
            type2schema(ty).unwrap()["allOf"],
            serde_json::json!([{"pattern": "[a-z]+"}, {"not": {"pattern": "x.*"}}])
        );
    }

    fn leaf_schema(src: &str, leaf: &str) -> Json {
        let schema = crate::schema::build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
        type2schema(schema.node_by_schema_path(leaf).unwrap().ty().unwrap()).unwrap()
    }

    const BOUNDS: &str = r#"
        module b {
          namespace "urn:b";
          prefix b;

          typedef short-string {
            type string {
              length "5..100";
            }
          }

          leaf count {
            type uint8;
          }
          leaf at-least-10 {
            type int8 {
              range "10..max";
            }
          }
          leaf name {
            type string;
          }
          leaf min-only {
            type string {
              length "10..max";
            }
          }
          leaf max-only {
            type string {
              length "min..8";
            }
          }
          leaf both {
            type string {
              length "1..10";
            }
          }
          leaf several {
            type string {
              length "min..5 | 10..20 | 30..max";
            }
          }
          leaf narrowed {
            type short-string {
              length "min..10";
            }
          }
          leaf blob {
            type binary;
          }
          leaf at-least-6-bytes {
            type binary {
              length "6..max";
            }
          }
        }
    "#;

    #[test]
    fn an_integer_always_has_both_bounds() {
        // A JSON number has no width or sign of its own, so the built-in
        // type's own bounds have to be spelled out.
        assert_eq!(
            leaf_schema(BOUNDS, "/b:count")["anyOf"],
            serde_json::json!([{"minimum": 0, "maximum": 255}])
        );
        assert_eq!(
            leaf_schema(BOUNDS, "/b:at-least-10")["anyOf"],
            serde_json::json!([{"minimum": 10, "maximum": 127}])
        );
    }

    #[test]
    fn a_length_bound_equal_to_the_builtins_own_is_left_out() {
        let any_of = |leaf: &str| leaf_schema(BOUNDS, leaf).get("anyOf").cloned();
        assert_eq!(any_of("/b:name"), None);
        assert_eq!(
            any_of("/b:min-only"),
            Some(serde_json::json!([{"minLength": 10}]))
        );
        assert_eq!(
            any_of("/b:max-only"),
            Some(serde_json::json!([{"maxLength": 8}]))
        );
        assert_eq!(
            any_of("/b:both"),
            Some(serde_json::json!([{"minLength": 1, "maxLength": 10}]))
        );
        assert_eq!(any_of("/b:blob"), None);
        assert_eq!(
            any_of("/b:at-least-6-bytes"),
            Some(serde_json::json!([{"minLength": 8}]))
        );
    }

    #[test]
    fn only_the_outermost_bounds_of_several_lengths_can_be_left_out() {
        assert_eq!(
            leaf_schema(BOUNDS, "/b:several")["anyOf"],
            serde_json::json!([
                {"maxLength": 5},
                {"minLength": 10, "maxLength": 20},
                {"minLength": 30}
            ])
        );
    }

    #[test]
    fn a_length_bound_is_compared_with_the_builtin_type_not_the_base_typedef() {
        // "min" here is the base typedef's own minimum, 5, which differs
        // from string's 0, so it is a real restriction and stays.
        assert_eq!(
            leaf_schema(BOUNDS, "/b:narrowed")["anyOf"],
            serde_json::json!([{"minLength": 5, "maxLength": 10}])
        );
    }

    #[test]
    fn a_bound_that_does_not_fit_in_a_json_number_is_an_error() {
        // 2^64 - 2 bytes fits in a u64, but its base64 length is 4/3 of
        // that and doesn't.
        let src = r#"
            module big {
              namespace "urn:big";
              prefix b;

              leaf blob {
                type binary {
                  length "18446744073709551614..max";
                }
              }
            }
        "#;
        let schema = crate::schema::build(&[src.to_string()], &[], &[], &[], &[]).unwrap();
        let err = to_json_schema_for_catalog(&schema, "yang").unwrap_err();
        assert!(err.contains("/big:blob"), "{err}");
        assert!(err.contains("24595658764946068820"), "{err}");
    }
}
