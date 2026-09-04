//! The SID-CBOR wire codec (RFC 9254 / RFC 9595), ported from
//! `support/yang-enc/yang-enc.rb`'s `type2cbor`/`type2json`/`json2cbor`/
//! `cbor2json`/`json_seq2cbor`/`cbor_seq2json`, cross-checked against
//! `sw-velocitydrive-devclient`'s `src/yang/yang-codec.ts` and
//! `client-lib/src/lm_yang.c`.

use ciborium::Value as Cbor;
use serde_json::Value as Json;

use crate::schema::{Builtin, IdentityId, Node, NodeId, Schema, TypeDef, TypeId};

#[derive(Debug)]
pub struct CodecError(pub String);
impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for CodecError {}
impl From<String> for CodecError {
    fn from(s: String) -> Self {
        CodecError(s)
    }
}
impl From<&str> for CodecError {
    fn from(s: &str) -> Self {
        CodecError(s.to_string())
    }
}

type R<T> = Result<T, CodecError>;

fn err(msg: impl Into<String>) -> CodecError {
    CodecError(msg.into())
}

/// The five wire content-format shapes `mup1cc` uses, matching
/// `yang-enc.rb`'s `content_format` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentFormat {
    /// A whole datastore tree, one CBOR map (aliases: response of `get`,
    /// request of `put`).
    Yang,
    Get,
    Put,
    /// CBOR sequence of bare IIDs (request) or single-entry maps
    /// (response).
    Fetch,
    /// CBOR sequence of single-entry maps only, both directions.
    Ipatch,
    Post,
}

impl ContentFormat {
    fn upper_name(self) -> &'static str {
        match self {
            ContentFormat::Yang => "YANG",
            ContentFormat::Get => "GET",
            ContentFormat::Put => "PUT",
            ContentFormat::Fetch => "FETCH",
            ContentFormat::Ipatch => "IPATCH",
            ContentFormat::Post => "POST",
        }
    }

    fn is_sequence(self) -> bool {
        matches!(self, ContentFormat::Fetch | ContentFormat::Ipatch | ContentFormat::Post)
    }
}

// ===========================================================================
// Type-level encode/decode
// ===========================================================================

/// Follow `leafref` chains to the type that actually governs wire
/// encoding (only needed for union-member dispatch heuristics below;
/// `type_to_cbor`/`type_to_json` themselves already delegate leafref
/// directly).
fn resolved_builtin(schema: &Schema, type_id: TypeId) -> (Builtin, TypeId) {
    let ty = schema.ty(type_id);
    if ty.builtin == Builtin::Leafref {
        if let Some(target) = ty.leafref_target {
            if let Some(t) = schema.node(target).type_id {
                return resolved_builtin(schema, t);
            }
        }
    }
    (ty.builtin, type_id)
}

fn json_number_to_i128(value: &Json) -> Option<i128> {
    value.as_i64().map(|v| v as i128).or_else(|| value.as_u64().map(|v| v as i128))
}

/// Encode a JSON value as CBOR for `type_id`. `in_union` tracks whether
/// this is (possibly nested) a member of a `union` -- enum/bits/
/// identityref/decimal64 wrap in a distinguishing CBOR tag only when
/// reached through a union (RFC 9254 6.3/6.6/6.7/6.10.1); otherwise they
/// use their plain (unwrapped) form since there's no ambiguity to
/// resolve.
pub fn type_to_cbor(schema: &Schema, type_id: TypeId, value: &Json, in_union: bool) -> R<Cbor> {
    let ty = schema.ty(type_id);
    match ty.builtin {
        Builtin::Int8 | Builtin::Int16 | Builtin::Int32 | Builtin::Uint8 | Builtin::Uint16 | Builtin::Uint32 => {
            let n = json_number_to_i128(value).ok_or_else(|| err(format!("expected an integer, got {value}")))?;
            Ok(Cbor::from(n as i64))
        }
        Builtin::Int64 => {
            let s = value.as_str().ok_or_else(|| err(format!("expected an int64 string, got {value}")))?;
            let n: i64 = s.parse().map_err(|_| err(format!("invalid int64 {s:?}")))?;
            Ok(Cbor::from(n))
        }
        Builtin::Uint64 => {
            let s = value.as_str().ok_or_else(|| err(format!("expected a uint64 string, got {value}")))?;
            let n: u64 = s.parse().map_err(|_| err(format!("invalid uint64 {s:?}")))?;
            Ok(Cbor::from(n))
        }
        Builtin::Boolean => Ok(Cbor::Bool(value.as_bool().ok_or_else(|| err(format!("expected a boolean, got {value}")))?)),
        Builtin::String => Ok(Cbor::Text(value.as_str().ok_or_else(|| err(format!("expected a string, got {value}")))?.to_string())),
        Builtin::Binary => {
            let s = value.as_str().ok_or_else(|| err(format!("expected a base64 string, got {value}")))?;
            Ok(Cbor::Bytes(base64_decode(s)?))
        }
        Builtin::Empty => Ok(Cbor::Null),
        Builtin::Decimal64 => encode_decimal64(ty, value, in_union),
        Builtin::Enumeration => encode_enum(ty, value, in_union),
        Builtin::Bits => encode_bits(ty, value, in_union),
        Builtin::Identityref => encode_identityref(schema, ty, value, in_union),
        Builtin::InstanceIdentifier => {
            let s = value.as_str().ok_or_else(|| err(format!("expected an instance-identifier string, got {value}")))?;
            resolve_iid(schema, s).map(|(_, cbor)| cbor)
        }
        Builtin::Leafref => match ty.leafref_target.and_then(|t| schema.node(t).type_id) {
            Some(target_type) => type_to_cbor(schema, target_type, value, in_union),
            // Unresolved (e.g. a relative leafref, not exercised in this
            // catalog): fall back to passthrough so encoding still
            // succeeds rather than hard-failing.
            None => json_scalar_passthrough_to_cbor(value),
        },
        Builtin::Union => {
            let member = pick_union_member_for_json(schema, ty, value)
                .ok_or_else(|| err(format!("no union member of type {type_id} matches value {value}")))?;
            type_to_cbor(schema, member, value, true)
        }
    }
}

fn json_scalar_passthrough_to_cbor(value: &Json) -> R<Cbor> {
    match value {
        Json::String(s) => Ok(Cbor::Text(s.clone())),
        Json::Bool(b) => Ok(Cbor::Bool(*b)),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Cbor::from(i))
            } else if let Some(u) = n.as_u64() {
                Ok(Cbor::from(u))
            } else {
                Ok(Cbor::Float(n.as_f64().unwrap_or(0.0)))
            }
        }
        Json::Null => Ok(Cbor::Null),
        other => Err(err(format!("cannot passthrough-encode {other}"))),
    }
}

/// Decode CBOR back to a JSON value for `type_id`.
pub fn type_to_json(schema: &Schema, type_id: TypeId, value: &Cbor, in_union: bool) -> R<Json> {
    let ty = schema.ty(type_id);
    match ty.builtin {
        Builtin::Int8 | Builtin::Int16 | Builtin::Int32 | Builtin::Uint8 | Builtin::Uint16 | Builtin::Uint32 => {
            let i = cbor_as_i128(value).ok_or_else(|| err(format!("expected an integer, got {value:?}")))?;
            Ok(Json::from(i as i64))
        }
        Builtin::Int64 => {
            let i = cbor_as_i128(value).ok_or_else(|| err(format!("expected an integer, got {value:?}")))?;
            Ok(Json::String((i as i64).to_string()))
        }
        Builtin::Uint64 => {
            let i = cbor_as_i128(value).ok_or_else(|| err(format!("expected an integer, got {value:?}")))?;
            Ok(Json::String((i as u64).to_string()))
        }
        Builtin::Boolean => Ok(Json::Bool(value.as_bool().ok_or_else(|| err(format!("expected a bool, got {value:?}")))?)),
        Builtin::String => Ok(Json::String(value.as_text().ok_or_else(|| err(format!("expected a string, got {value:?}")))?.to_string())),
        Builtin::Binary => {
            let bytes = value.as_bytes().ok_or_else(|| err(format!("expected bytes, got {value:?}")))?;
            Ok(Json::String(base64_encode(bytes)))
        }
        Builtin::Empty => Ok(Json::Array(vec![Json::Null])),
        Builtin::Decimal64 => decode_decimal64(ty, value, in_union),
        Builtin::Enumeration => decode_enum(ty, value, in_union),
        Builtin::Bits => decode_bits(ty, value, in_union),
        Builtin::Identityref => decode_identityref(schema, ty, value, in_union),
        Builtin::InstanceIdentifier => cbor_to_iid_string(schema, value).map(Json::String),
        Builtin::Leafref => match ty.leafref_target.and_then(|t| schema.node(t).type_id) {
            Some(target_type) => type_to_json(schema, target_type, value, in_union),
            None => cbor_scalar_passthrough_to_json(value),
        },
        Builtin::Union => {
            let member = pick_union_member_for_cbor(schema, ty, value)
                .ok_or_else(|| err(format!("no union member of type {type_id} matches value {value:?}")))?;
            type_to_json(schema, member, value, true)
        }
    }
}

fn cbor_scalar_passthrough_to_json(value: &Cbor) -> R<Json> {
    match value {
        Cbor::Text(s) => Ok(Json::String(s.clone())),
        Cbor::Bool(b) => Ok(Json::Bool(*b)),
        Cbor::Integer(_) => Ok(Json::from(cbor_as_i128(value).unwrap_or(0) as i64)),
        Cbor::Float(f) => Ok(serde_json::Number::from_f64(*f).map(Json::Number).unwrap_or(Json::Null)),
        Cbor::Null => Ok(Json::Null),
        other => Err(err(format!("cannot passthrough-decode {other:?}"))),
    }
}

fn cbor_as_i128(value: &Cbor) -> Option<i128> {
    match value {
        Cbor::Integer(i) => i128::try_from(*i).ok(),
        _ => None,
    }
}

// -- decimal64 ---------------------------------------------------------

fn encode_decimal64(ty: &TypeDef, value: &Json, in_union: bool) -> R<Cbor> {
    let fraction_digits = ty.fraction_digits.ok_or_else(|| err("decimal64 type missing fraction-digits"))? as usize;
    let s = value.as_str().ok_or_else(|| err(format!("expected a decimal64 string, got {value}")))?;
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => (-1i128, r),
        None => (1i128, s),
    };
    let (int_part, frac_part) = rest.split_once('.').unwrap_or((rest, ""));
    let int_part = if int_part.is_empty() { "0" } else { int_part };
    if frac_part.len() > fraction_digits {
        return Err(err(format!("{s:?} has more than {fraction_digits} fractional digits")));
    }
    let padded_frac = format!("{frac_part:0<fraction_digits$}");
    let mantissa: i128 = format!("{int_part}{padded_frac}").parse().map_err(|_| err(format!("invalid decimal64 {s:?}")))?;
    let mantissa = sign * mantissa;
    let tag = Cbor::Tag(4, Box::new(Cbor::Array(vec![Cbor::from(-(fraction_digits as i64)), Cbor::from(mantissa as i64)])));
    if in_union {
        Ok(tag)
    } else {
        Ok(tag)
    }
}

fn decode_decimal64(_ty: &TypeDef, value: &Cbor, _in_union: bool) -> R<Json> {
    let (exp, mantissa) = decimal64_tag_parts(value)?;
    let f = (mantissa as f64) * 10f64.powi(exp as i32);
    Ok(Json::String(format_ruby_float(f)))
}

fn decimal64_tag_parts(value: &Cbor) -> R<(i64, i128)> {
    match value {
        Cbor::Tag(4, inner) => match inner.as_array() {
            Some(arr) if arr.len() == 2 => {
                let exp = cbor_as_i128(&arr[0]).ok_or_else(|| err("decimal64 exponent not an integer"))? as i64;
                let mantissa = cbor_as_i128(&arr[1]).ok_or_else(|| err("decimal64 mantissa not an integer"))?;
                Ok((exp, mantissa))
            }
            _ => Err(err(format!("malformed decimal64 tag content {inner:?}"))),
        },
        other => Err(err(format!("expected a decimal64 tag(4,...), got {other:?}"))),
    }
}

fn format_ruby_float(f: f64) -> String {
    if f.is_finite() && f == f.trunc() && f.abs() < 1e15 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

// -- enumeration ---------------------------------------------------------

fn encode_enum(ty: &TypeDef, value: &Json, in_union: bool) -> R<Cbor> {
    let name = value.as_str().ok_or_else(|| err(format!("expected an enum name, got {value}")))?;
    let e = ty.enums.iter().find(|e| e.name == name).ok_or_else(|| err(format!("unknown enum value {name:?}")))?;
    if in_union {
        Ok(Cbor::Tag(44, Box::new(Cbor::Text(name.to_string()))))
    } else {
        Ok(Cbor::from(e.value))
    }
}

fn decode_enum(ty: &TypeDef, value: &Cbor, in_union: bool) -> R<Json> {
    if in_union {
        if let Cbor::Tag(44, inner) = value {
            let name = inner.as_text().ok_or_else(|| err("enum tag content not text"))?;
            return Ok(Json::String(name.to_string()));
        }
    }
    let n = cbor_as_i128(value).ok_or_else(|| err(format!("expected an enum ordinal, got {value:?}")))?;
    match ty.enums.iter().find(|e| e.value as i128 == n) {
        Some(e) => Ok(Json::String(e.name.clone())),
        None => {
            eprintln!("yang: unknown enumeration ordinal {n}; decoding as raw value (YANG model behind firmware?)");
            Ok(Json::from(n as i64))
        }
    }
}

// -- bits (RFC 9254 6.7) -------------------------------------------------

fn encode_bits(ty: &TypeDef, value: &Json, in_union: bool) -> R<Cbor> {
    let names: Vec<&str> = match value {
        Json::String(s) => s.split_whitespace().collect(),
        Json::Array(items) => items.iter().filter_map(|v| v.as_str()).collect(),
        other => return Err(err(format!("expected a bits string/array, got {other}"))),
    };
    if in_union {
        return Ok(Cbor::Tag(43, Box::new(Cbor::Text(names.join(" ")))));
    }

    let mut positions = Vec::new();
    for name in &names {
        let bit = ty.bits.iter().find(|b| b.name == *name).ok_or_else(|| err(format!("unknown bit {name:?}")))?;
        positions.push(bit.position);
    }
    positions.sort_unstable();
    if positions.is_empty() {
        return Ok(Cbor::Array(vec![]));
    }

    let max_byte = (positions.last().unwrap() / 8) as usize;
    let mut bytes = vec![0u8; max_byte + 1];
    for &p in &positions {
        bytes[(p / 8) as usize] |= 1 << (p % 8);
    }
    let items = bits_bytes_to_spans(&bytes);
    if items.len() == 1 {
        if let Cbor::Bytes(b) = &items[0] {
            return Ok(Cbor::Bytes(b.clone()));
        }
    }
    Ok(Cbor::Array(items))
}

/// Split a packed bit-array into the RFC 9254 6.7 span/gap representation:
/// contiguous non-zero-byte spans as byte strings, separated by an
/// integer byte-count for runs of zero bytes (including a leading gap if
/// `bytes` starts with zero bytes).
fn bits_bytes_to_spans(bytes: &[u8]) -> Vec<Cbor> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut pending_gap = 0u32;
    while i < bytes.len() {
        if bytes[i] == 0 {
            pending_gap += 1;
            i += 1;
            continue;
        }
        if !out.is_empty() || pending_gap > 0 {
            if pending_gap > 0 {
                out.push(Cbor::from(pending_gap));
            }
        }
        pending_gap = 0;
        let start = i;
        while i < bytes.len() && bytes[i] != 0 {
            i += 1;
        }
        out.push(Cbor::Bytes(bytes[start..i].to_vec()));
    }
    out
}

fn decode_bits(ty: &TypeDef, value: &Cbor, in_union: bool) -> R<Json> {
    if in_union {
        if let Cbor::Tag(43, inner) = value {
            let s = inner.as_text().ok_or_else(|| err("bits tag content not text"))?;
            return Ok(Json::String(s.to_string()));
        }
    }

    let spans: Vec<&Cbor> = match value {
        Cbor::Bytes(_) => vec![value],
        Cbor::Array(items) => items.iter().collect(),
        other => return Err(err(format!("expected bits bytes/array, got {other:?}"))),
    };

    let mut names = Vec::new();
    let mut byte_offset: u32 = 0;
    for item in spans {
        match item {
            Cbor::Integer(_) => {
                byte_offset += cbor_as_i128(item).unwrap_or(0) as u32;
            }
            Cbor::Bytes(b) => {
                for (i, byte) in b.iter().enumerate() {
                    for bit in 0..8u32 {
                        if byte & (1 << bit) != 0 {
                            let pos = (byte_offset + i as u32) * 8 + bit;
                            if let Some(bitdef) = ty.bits.iter().find(|x| x.position == pos) {
                                names.push(bitdef.name.clone());
                            }
                        }
                    }
                }
                byte_offset += b.len() as u32;
            }
            other => return Err(err(format!("unexpected bits span element {other:?}"))),
        }
    }
    Ok(Json::String(names.join(" ")))
}

// -- identityref (RFC 9254 6.10 / RFC 9595) ------------------------------

fn all_identity_bases(schema: &Schema, ty: &TypeDef) -> Option<std::collections::HashSet<IdentityId>> {
    let mut iter = ty.identity_bases.iter();
    let first = *iter.next()?;
    let mut set = schema.derived_from(first);
    for &b in iter {
        let other = schema.derived_from(b);
        set.retain(|id| other.contains(id));
    }
    Some(set)
}

fn encode_identityref(schema: &Schema, ty: &TypeDef, value: &Json, in_union: bool) -> R<Cbor> {
    let s = value.as_str().ok_or_else(|| err(format!("expected an identity name, got {value}")))?;
    let candidates = all_identity_bases(schema, ty).ok_or_else(|| err("identityref type has no base"))?;
    let source_module = ty.source_module.as_deref().unwrap_or("");

    let found = candidates.into_iter().find(|&id| {
        let identity = schema.identity(id);
        match s.split_once(':') {
            Some((m, n)) => identity.module == m && identity.name == n,
            None => identity.module == source_module && identity.name == s,
        }
    });
    let id = found.ok_or_else(|| err(format!("unknown identity value {s:?}")))?;
    let sid = schema.identity(id).sid.ok_or_else(|| err(format!("identity {s:?} has no SID")))?;
    if in_union {
        Ok(Cbor::Tag(45, Box::new(Cbor::from(sid))))
    } else {
        Ok(Cbor::from(sid))
    }
}

fn decode_identityref(schema: &Schema, _ty: &TypeDef, value: &Cbor, in_union: bool) -> R<Json> {
    let sid = if in_union {
        match value {
            Cbor::Tag(45, inner) => cbor_as_i128(inner).ok_or_else(|| err("identityref tag content not an integer"))?,
            other => return Err(err(format!("expected identityref tag(45,...), got {other:?}"))),
        }
    } else {
        cbor_as_i128(value).ok_or_else(|| err(format!("expected an identity SID, got {value:?}")))?
    };
    let identity = schema.identities.iter().find(|i| i.sid == Some(sid as i64)).ok_or_else(|| err(format!("unknown identity SID {sid}")))?;
    // Decode always qualifies with the defining module, confirmed against
    // real device output (e.g. "ietf-routing:ipv4",
    // "ietf-datastores:startup") even when that module matches the
    // identityref's own declaring module -- unlike encode, which accepts
    // (and the real Ruby tool's own request encoding uses) the bare form
    // when they match.
    Ok(Json::String(format!("{}:{}", identity.module, identity.name)))
}

// -- union member dispatch (encoding-focused heuristic; see module docs) -

fn pick_union_member_for_json(schema: &Schema, ty: &TypeDef, value: &Json) -> Option<TypeId> {
    let members: Vec<TypeId> = ty.union_members.clone();

    for &m in &members {
        if resolved_builtin(schema, m).0 == Builtin::Identityref {
            if let Json::String(s) = value {
                let mt = schema.ty(resolved_builtin(schema, m).1);
                if let Some(candidates) = all_identity_bases(schema, mt) {
                    let source_module = mt.source_module.as_deref().unwrap_or("");
                    let matches = candidates.into_iter().any(|id| {
                        let identity = schema.identity(id);
                        match s.split_once(':') {
                            Some((mm, n)) => identity.module == mm && identity.name == n,
                            None => identity.module == source_module && identity.name == s.as_str(),
                        }
                    });
                    if matches {
                        return Some(m);
                    }
                }
            }
        }
    }
    for &m in &members {
        if resolved_builtin(schema, m).0 == Builtin::Enumeration {
            if let Json::String(s) = value {
                let mt = schema.ty(resolved_builtin(schema, m).1);
                if mt.enums.iter().any(|e| &e.name == s) {
                    return Some(m);
                }
            }
        }
    }
    for &m in &members {
        if resolved_builtin(schema, m).0 == Builtin::Bits {
            let names: Option<Vec<&str>> = match value {
                Json::String(s) => Some(s.split_whitespace().collect()),
                Json::Array(items) => Some(items.iter().filter_map(|v| v.as_str()).collect()),
                _ => None,
            };
            if let Some(names) = names {
                let mt = schema.ty(resolved_builtin(schema, m).1);
                if !names.is_empty() && names.iter().all(|n| mt.bits.iter().any(|b| &b.name == n)) {
                    return Some(m);
                }
            }
        }
    }
    for &m in &members {
        if resolved_builtin(schema, m).0 == Builtin::Decimal64 {
            if let Json::String(s) = value {
                if s.trim_start_matches('-').chars().all(|c| c.is_ascii_digit() || c == '.') && s.contains('.') {
                    return Some(m);
                }
            }
        }
    }
    for &m in &members {
        let b = resolved_builtin(schema, m).0;
        if b.is_64bit_int() {
            if let Json::String(s) = value {
                let ok = if b == Builtin::Int64 { s.parse::<i64>().is_ok() } else { s.parse::<u64>().is_ok() };
                if ok {
                    return Some(m);
                }
            }
        }
    }
    for &m in &members {
        match (resolved_builtin(schema, m).0, value) {
            (Builtin::Boolean, Json::Bool(_)) => return Some(m),
            (Builtin::Int8 | Builtin::Int16 | Builtin::Int32 | Builtin::Uint8 | Builtin::Uint16 | Builtin::Uint32, Json::Number(_)) => return Some(m),
            (Builtin::String | Builtin::Binary | Builtin::InstanceIdentifier, Json::String(_)) => return Some(m),
            (Builtin::Empty, Json::Array(a)) if a.len() == 1 && a[0].is_null() => return Some(m),
            _ => {}
        }
    }
    members.first().copied()
}

fn pick_union_member_for_cbor(schema: &Schema, ty: &TypeDef, value: &Cbor) -> Option<TypeId> {
    let members: Vec<TypeId> = ty.union_members.clone();
    let tag_builtin = match value {
        Cbor::Tag(4, _) => Some(Builtin::Decimal64),
        Cbor::Tag(43, _) => Some(Builtin::Bits),
        Cbor::Tag(44, _) => Some(Builtin::Enumeration),
        Cbor::Tag(45, _) => Some(Builtin::Identityref),
        _ => None,
    };
    if let Some(b) = tag_builtin {
        if let Some(&m) = members.iter().find(|&&m| resolved_builtin(schema, m).0 == b) {
            return Some(m);
        }
    }
    match value {
        Cbor::Integer(_) => {
            let n = cbor_as_i128(value).unwrap_or(0);
            let fits_32 = (i32::MIN as i128..=u32::MAX as i128).contains(&n);
            if !fits_32 {
                if let Some(&m) = members.iter().find(|&&m| resolved_builtin(schema, m).0.is_64bit_int()) {
                    return Some(m);
                }
            }
            members.iter().copied().find(|&m| {
                matches!(
                    resolved_builtin(schema, m).0,
                    Builtin::Int8 | Builtin::Int16 | Builtin::Int32 | Builtin::Uint8 | Builtin::Uint16 | Builtin::Uint32 | Builtin::Int64 | Builtin::Uint64
                )
            })
        }
        Cbor::Text(_) => members
            .iter()
            .copied()
            .find(|&m| matches!(resolved_builtin(schema, m).0, Builtin::String | Builtin::InstanceIdentifier)),
        Cbor::Bytes(_) => members.iter().copied().find(|&m| resolved_builtin(schema, m).0 == Builtin::Binary),
        Cbor::Bool(_) => members.iter().copied().find(|&m| resolved_builtin(schema, m).0 == Builtin::Boolean),
        Cbor::Null => members.iter().copied().find(|&m| resolved_builtin(schema, m).0 == Builtin::Empty),
        _ => None,
    }
    .or_else(|| members.first().copied())
}

// ===========================================================================
// Instance-identifiers: "/mod:top/child[key='v']/..." <-> SID (+ keys)
// ===========================================================================

struct IidSegment<'a> {
    name: &'a str,
    keys: Vec<(&'a str, &'a str)>,
}

fn split_outside_brackets(path: &str) -> Vec<&str> {
    let mut segs = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in path.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            '/' if depth == 0 => {
                segs.push(&path[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    segs.push(&path[start..]);
    segs.into_iter().filter(|s| !s.is_empty()).collect()
}

fn parse_iid_segment(seg: &str) -> R<IidSegment<'_>> {
    let bracket = seg.find('[');
    let (name, rest) = match bracket {
        Some(i) => (&seg[..i], &seg[i..]),
        None => (seg, ""),
    };
    let mut keys = Vec::new();
    let mut r = rest;
    while let Some(open) = r.find('[') {
        let close = r[open..].find(']').ok_or_else(|| err(format!("unbalanced '[' in IID segment {seg:?}")))?;
        let inner = &r[open + 1..open + close];
        let (k, v) = inner.split_once('=').ok_or_else(|| err(format!("malformed key predicate {inner:?} in {seg:?}")))?;
        let v = v.trim();
        let v = v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')).or_else(|| v.strip_prefix('"').and_then(|v| v.strip_suffix('"'))).unwrap_or(v);
        keys.push((k, v));
        r = &r[open + close + 1..];
    }
    Ok(IidSegment { name, keys })
}

/// Convert an IID key value's raw text (always a string in the path
/// syntax) into the JSON shape `type_to_cbor` expects for that key leaf's
/// type -- matching `convert_iid_key_value`: only int/uint/boolean/empty
/// get a native conversion; every other type (including decimal64,
/// identityref, and plain string) is passed through as a JSON string
/// as-is.
fn convert_iid_key_value(schema: &Schema, type_id: TypeId, raw: &str) -> R<Json> {
    match resolved_builtin(schema, type_id).0 {
        // int64/uint64 stay a JSON string -- `type_to_cbor` expects the
        // RFC 7951 64-bit string convention, not a JSON number.
        Builtin::Int64 | Builtin::Uint64 => Ok(Json::String(raw.to_string())),
        Builtin::Int8 | Builtin::Int16 | Builtin::Int32 => {
            let n: i64 = raw.parse().map_err(|_| err(format!("invalid integer key value {raw:?}")))?;
            Ok(Json::from(n))
        }
        Builtin::Uint8 | Builtin::Uint16 | Builtin::Uint32 => {
            let n: u64 = raw.parse().map_err(|_| err(format!("invalid integer key value {raw:?}")))?;
            Ok(Json::from(n))
        }
        Builtin::Boolean => match raw {
            "true" => Ok(Json::Bool(true)),
            "false" => Ok(Json::Bool(false)),
            other => Err(err(format!("invalid boolean key value {other:?}"))),
        },
        Builtin::Empty if raw == "[null]" => Ok(Json::Array(vec![Json::Null])),
        _ => Ok(Json::String(raw.to_string())),
    }
}

/// Encode an instance-identifier path string to its CBOR representation
/// (bare SID, or `[SID, key1, key2, ...]` for a list entry) and return the
/// schema node it addresses.
pub fn resolve_iid(schema: &Schema, path: &str) -> R<(NodeId, Cbor)> {
    let segs = split_outside_brackets(path);
    let mut cur = schema.root;
    let mut last_sid = 0i64;
    // Keys accumulate from *every* list ancestor traversed along the
    // path, not just the final segment: a path descending into a leaf
    // underneath a keyed list entry still needs that list's keys to fully
    // address the instance (e.g. ".../rule-list[id='1']/rx-count" encodes
    // as `[rx-count's SID, 1]`, not a bare SID).
    let mut all_keys: Vec<Cbor> = Vec::new();

    for raw_seg in segs {
        let seg = parse_iid_segment(raw_seg)?;
        let child = schema.find_child(cur, seg.name).ok_or_else(|| err(format!("could not find {} in schema tree ({path:?})", seg.name)))?;
        let node = schema.node(child);
        let sid = node.sid.ok_or_else(|| err(format!("{} has no SID", seg.name)))?;

        let mut ordered_keys = Vec::new();
        if !node.keys.is_empty() {
            for key_name in &node.keys {
                if let Some((_, v)) = seg.keys.iter().find(|(k, _)| k == key_name) {
                    let key_child = schema
                        .find_child(child, key_name)
                        .ok_or_else(|| err(format!("could not find key {key_name:?} in schema tree ({path:?})")))?;
                    let key_type = schema.node(key_child).type_id.ok_or_else(|| err(format!("key {key_name:?} has no type")))?;
                    let json_v = convert_iid_key_value(schema, key_type, v)?;
                    let cbor_v = type_to_cbor(schema, key_type, &json_v, false)?;
                    ordered_keys.push(cbor_v);
                }
            }
            for (k, _) in &seg.keys {
                if !node.keys.iter().any(|nk| nk == k) {
                    return Err(err(format!("could not find key: {k:?} in schema tree ({path:?})")));
                }
            }
        } else if !seg.keys.is_empty() {
            return Err(err(format!("{} is not a list; unexpected keys in {path:?}", seg.name)));
        }

        cur = child;
        last_sid = sid;
        all_keys.extend(ordered_keys);
    }

    let cbor = if all_keys.is_empty() {
        Cbor::from(last_sid)
    } else {
        let mut arr = vec![Cbor::from(last_sid)];
        arr.extend(all_keys);
        Cbor::Array(arr)
    };
    Ok((cur, cbor))
}

/// Split a decoded IID CBOR value into (absolute SID, key CBOR values).
fn split_iid_cbor(value: &Cbor) -> R<(i64, Vec<Cbor>)> {
    match value {
        Cbor::Integer(_) => Ok((cbor_as_i128(value).ok_or_else(|| err("invalid SID"))? as i64, Vec::new())),
        Cbor::Array(items) => {
            let mut it = items.iter();
            let sid = it.next().and_then(cbor_as_i128).ok_or_else(|| err(format!("IID array {items:?} missing a leading SID")))? as i64;
            Ok((sid, it.cloned().collect()))
        }
        other => Err(err(format!("expected a bare SID or [SID, keys...], got {other:?}"))),
    }
}

/// Decode a CBOR IID back to `find_node_from_sid`'s node plus a rebuilt
/// path string (single-quoted key values, canonical form -- not
/// necessarily byte-identical to whatever original string produced the
/// SID, matching `iid2json`'s behavior).
pub fn decode_iid(schema: &Schema, value: &Cbor) -> R<(NodeId, String)> {
    let (sid, mut keys) = split_iid_cbor(value)?;
    let node = *schema.sid_index.get(&sid).ok_or_else(|| err(format!("unknown SID {sid}")))?;
    let path = node_path_string(schema, node, &mut keys)?;
    Ok((node, path))
}

fn node_path_string(schema: &Schema, node: NodeId, keys: &mut Vec<Cbor>) -> R<String> {
    let mut chain = Vec::new();
    let mut cur = Some(node);
    while let Some(n) = cur {
        chain.push(n);
        cur = schema.node(n).parent;
    }
    chain.reverse();

    let mut path = String::new();
    for &n in &chain {
        let nd = schema.node(n);
        if nd.kw == "module" {
            continue;
        }
        path.push('/');
        path.push_str(&nd.name);
        if !nd.keys.is_empty() {
            // Every list ancestor along the chain consumes its keys from
            // the front, in the same outer-to-inner order `resolve_iid`
            // accumulates them -- not just the final target node.
            for key_name in &nd.keys {
                let key_child = schema.find_child(n, key_name).ok_or_else(|| err(format!("list {} missing key leaf {key_name:?}", nd.name)))?;
                let key_type = schema.node(key_child).type_id.ok_or_else(|| err(format!("key {key_name:?} has no type")))?;
                if keys.is_empty() {
                    break;
                }
                let cbor_v = keys.remove(0);
                let json_v = type_to_json(schema, key_type, &cbor_v, false)?;
                let v = json_v.as_str().map(|s| s.to_string()).unwrap_or_else(|| json_v.to_string());
                path.push_str(&format!("[{key_name}='{v}']"));
            }
        }
    }
    Ok(path)
}

fn cbor_to_iid_string(schema: &Schema, value: &Cbor) -> R<String> {
    decode_iid(schema, value).map(|(_, s)| s)
}

// ===========================================================================
// Node-body (container/list/leaf/rpc-action) encode/decode
// ===========================================================================

const IMPLICIT_INPUT_OUTPUT_BASE: &[&str] = &["rpc", "action"];

fn effective_delta_base(schema: &Schema, node: NodeId) -> i64 {
    let n = schema.node(node);
    if (n.kw == "input" || n.kw == "output") && n.parent.is_some() {
        let parent = n.parent.unwrap();
        if IMPLICIT_INPUT_OUTPUT_BASE.contains(&schema.node(parent).kw.as_str()) {
            if let Some(sid) = schema.node(parent).sid {
                return sid;
            }
        }
    }
    n.sid.unwrap_or(0)
}

fn encode_leaf_value(schema: &Schema, node: &Node, value: &Json) -> R<Cbor> {
    let type_id = node.type_id.ok_or_else(|| err(format!("{} has no type", node.name)))?;
    type_to_cbor(schema, type_id, value, false)
}

fn decode_leaf_value(schema: &Schema, node: &Node, value: &Cbor) -> R<Json> {
    let type_id = node.type_id.ok_or_else(|| err(format!("{} has no type", node.name)))?;
    type_to_json(schema, type_id, value, false)
}

/// Encode one container's (or one list entry's, or one rpc input/output's)
/// body: a JSON object of child-name -> value into a CBOR map keyed by
/// delta-SID.
fn encode_body(schema: &Schema, scope: NodeId, obj: &serde_json::Map<String, Json>, cf: ContentFormat) -> R<Cbor> {
    let base = effective_delta_base(schema, scope);
    // Real encoder output orders map entries by schema declaration order
    // (not ascending delta-SID -- an augmented field can sit before an
    // earlier-declared one with a lower SID -- and not JSON input order)
    // -- *except* that for a list entry, the key leaves come first, in
    // the exact order the `key` statement declared them (matching
    // `resolve_iid`'s key-array order), ahead of every non-key field
    // regardless of declaration position. Matched against the child's
    // full `name` (already qualified "module:local" for a node augmented
    // in from a different module, bare otherwise; RFC 7951 4.2's
    // JSON-member-name rule).
    let keys = &schema.node(scope).keys;
    let ordered_children = schema.node(scope).children.iter().copied().filter(|c| !keys.iter().any(|k| schema.node(*c).local_name() == k));
    let mut entries = Vec::new();
    for key_name in keys {
        let Some(child) = schema.find_child(scope, key_name) else { continue };
        let name = schema.node(child).name.as_str();
        let Some(val) = obj.get(name) else { continue };
        let child_sid = schema.node(child).sid.ok_or_else(|| err(format!("{name:?} has no SID")))?;
        let cbor_val = encode_node_value(schema, child, val, cf)?;
        entries.push((Cbor::from(child_sid - base), cbor_val));
    }
    for child in ordered_children {
        let name = schema.node(child).name.as_str();
        let Some(val) = obj.get(name) else { continue };
        let child_sid = schema.node(child).sid.ok_or_else(|| err(format!("{name:?} has no SID")))?;
        let cbor_val = encode_node_value(schema, child, val, cf)?;
        entries.push((Cbor::from(child_sid - base), cbor_val));
    }
    let known: std::collections::HashSet<&str> = schema.node(scope).children.iter().map(|&c| schema.node(c).name.as_str()).collect();
    for key in obj.keys() {
        if !known.contains(key.as_str()) {
            return Err(err(format!("unknown child {key:?} of {}", schema.node(scope).name)));
        }
    }
    Ok(Cbor::Map(entries))
}

fn decode_body(schema: &Schema, scope: NodeId, value: &Cbor, cf: ContentFormat) -> R<Json> {
    let base = effective_delta_base(schema, scope);
    let map = value.as_map().ok_or_else(|| err(format!("expected a CBOR map for {}, got {value:?}", schema.node(scope).name)))?;
    let mut obj = serde_json::Map::new();
    for (k, v) in map {
        let delta = cbor_as_i128(k).ok_or_else(|| err("map key is not an integer"))? as i64;
        let child = *schema.sid_index.get(&(delta + base)).ok_or_else(|| err(format!("unknown SID {}", delta + base)))?;
        let name = schema.node(child).name.clone();
        obj.insert(name, decode_node_value(schema, child, v, cf)?);
    }
    Ok(Json::Object(obj))
}

/// Encode a single JSON value for `node` (whatever kind it is) into CBOR.
pub fn encode_node_value(schema: &Schema, node: NodeId, value: &Json, cf: ContentFormat) -> R<Cbor> {
    let n = schema.node(node);
    match n.kw.as_str() {
        "leaf" => encode_leaf_value(schema, n, value),
        "leaf-list" => {
            let items = value.as_array().ok_or_else(|| err(format!("{} (leaf-list) expects an array, got {value}", n.name)))?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(encode_leaf_value(schema, n, item)?);
            }
            Ok(Cbor::Array(out))
        }
        "container" | "input" | "output" => {
            let obj = value.as_object().ok_or_else(|| err(format!("{} (container) expects a map, got {value}", n.name)))?;
            encode_body(schema, node, obj, cf)
        }
        "list" => match value {
            Json::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let obj = item.as_object().ok_or_else(|| err(format!("{} entry expects a map, got {item}", n.name)))?;
                    out.push(encode_body(schema, node, obj, cf)?);
                }
                Ok(Cbor::Array(out))
            }
            Json::Object(obj) if cf.is_sequence() => encode_body(schema, node, obj, cf),
            other => Err(err(format!("{} (list) expects an array{}, got {other}", n.name, if cf.is_sequence() { " or a single-entry map" } else { "" }))),
        },
        "rpc" | "action" => {
            // On the wire this is always flat (the input/output params
            // directly, keyed by delta from the *rpc's* own SID -- see
            // `effective_delta_base`). As user-facing YAML/JSON input,
            // though, a single `input:`/`output:` wrapper key disambiguates
            // which side is meant; unwrap it if present.
            let obj = value.as_object().ok_or_else(|| err(format!("{} expects a map, got {value}", n.name)))?;
            if obj.len() == 1 {
                for side_name in ["input", "output"] {
                    if let Some(inner) = obj.get(side_name) {
                        let side_node = schema.find_child(node, side_name).ok_or_else(|| err(format!("{} has no {side_name}", n.name)))?;
                        let inner_obj = inner.as_object().ok_or_else(|| err(format!("{} {side_name} expects a map, got {inner}", n.name)))?;
                        return encode_body(schema, side_node, inner_obj, cf);
                    }
                }
            }
            let side = schema.find_child(node, "input").unwrap_or(node);
            encode_body(schema, side, obj, cf)
        }
        other => Err(err(format!("unsupported schema-node kind {other:?} for {}", n.name))),
    }
}

pub fn decode_node_value(schema: &Schema, node: NodeId, value: &Cbor, cf: ContentFormat) -> R<Json> {
    let n = schema.node(node);
    match n.kw.as_str() {
        "leaf" => decode_leaf_value(schema, n, value),
        "leaf-list" => {
            let items = value.as_array().ok_or_else(|| err(format!("{} (leaf-list) expects a CBOR array, got {value:?}", n.name)))?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(decode_leaf_value(schema, n, item)?);
            }
            Ok(Json::Array(out))
        }
        "container" | "input" | "output" => decode_body(schema, node, value, cf),
        "list" => match value {
            Cbor::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(decode_body(schema, node, item, cf)?);
                }
                Ok(Json::Array(out))
            }
            Cbor::Map(_) if cf.is_sequence() => decode_body(schema, node, value, cf),
            other => Err(err(format!("{} (list) expects a CBOR array or map, got {other:?}", n.name))),
        },
        "rpc" | "action" => {
            let side_out = schema.find_child(node, "output");
            let side_in = schema.find_child(node, "input");
            // A response payload doesn't say whether keys belong to
            // input or output; try output first (the common decode
            // direction for a POST response), falling back to input.
            if let Some(out_node) = side_out {
                if let Ok(v) = decode_body(schema, out_node, value, cf) {
                    return Ok(v);
                }
            }
            if let Some(in_node) = side_in {
                return decode_body(schema, in_node, value, cf);
            }
            decode_body(schema, node, value, cf)
        }
        other => Err(err(format!("unsupported schema-node kind {other:?} for {}", n.name))),
    }
}

// ===========================================================================
// Top-level entry points
// ===========================================================================

fn cbor_to_bytes(value: &Cbor) -> R<Vec<u8>> {
    let mut buf = Vec::new();
    ciborium::into_writer(value, &mut buf).map_err(|e| err(format!("CBOR encode error: {e}")))?;
    Ok(buf)
}

fn cbor_seq_from_bytes(bytes: &[u8]) -> R<Vec<Cbor>> {
    let mut cursor = std::io::Cursor::new(bytes);
    let mut items = Vec::new();
    while (cursor.position() as usize) < bytes.len() {
        let v: Cbor = ciborium::from_reader(&mut cursor).map_err(|e| err(format!("CBOR decode error: {e}")))?;
        items.push(v);
    }
    Ok(items)
}

/// Validate one `fetch`/`ipatch`/`post` sequence entry's shape, matching
/// `validate_instance_entry!`'s exact error wording.
fn validate_instance_entry(cf: ContentFormat, entry: &Json) -> R<()> {
    let kind = match entry {
        Json::Object(_) => return validate_single_key(cf, entry),
        Json::Array(_) => "array",
        Json::Null => "null",
        Json::Bool(_) => "Boolean",
        Json::Number(_) => "Numeric",
        Json::String(_) => "String",
    };
    Err(err(format!("{} entry must be a map of a single instance path to its value, got {kind}", cf.upper_name())))
}

fn validate_single_key(cf: ContentFormat, entry: &Json) -> R<()> {
    let obj = entry.as_object().unwrap();
    if obj.len() != 1 {
        let keys: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
        return Err(err(format!(
            "{} entry must have exactly one key/value pair (the instance path and its value), got {}: {}",
            cf.upper_name(),
            obj.len(),
            keys.join(", ")
        )));
    }
    Ok(())
}

/// `json_seq2cbor`: encode a sequence of fetch/ipatch/post entries into a
/// concatenated CBOR-sequence byte string.
pub fn json_seq_to_cbor(schema: &Schema, items: &[Json], cf: ContentFormat) -> R<Vec<u8>> {
    let mut out = Vec::new();
    for item in items {
        let cbor_item = match cf {
            ContentFormat::Fetch => match item {
                Json::String(path) => resolve_iid(schema, path)?.1,
                Json::Object(map) if map.len() == 1 => {
                    let (path, val) = map.iter().next().unwrap();
                    let (target, iid) = resolve_iid(schema, path)?;
                    let encoded_val = if val.is_null() { Cbor::Null } else { encode_node_value(schema, target, val, cf)? };
                    Cbor::Map(vec![(iid, encoded_val)])
                }
                other => return Err(validate_entry_err(cf, other)),
            },
            ContentFormat::Ipatch | ContentFormat::Post => {
                validate_instance_entry(cf, item)?;
                let map = item.as_object().unwrap();
                let (path, val) = map.iter().next().unwrap();
                let (target, iid) = resolve_iid(schema, path)?;
                let encoded_val = if val.is_null() { Cbor::Null } else { encode_node_value(schema, target, val, cf)? };
                Cbor::Map(vec![(iid, encoded_val)])
            }
            ContentFormat::Yang | ContentFormat::Get | ContentFormat::Put => {
                return Err(err(format!("{:?} does not use sequence encoding", cf)));
            }
        };
        out.extend(cbor_to_bytes(&cbor_item)?);
    }
    Ok(out)
}

fn validate_entry_err(cf: ContentFormat, entry: &Json) -> CodecError {
    let kind = match entry {
        Json::Array(_) => "array",
        Json::Null => "null",
        Json::Bool(_) => "Boolean",
        Json::Number(_) => "Numeric",
        Json::String(_) => "String",
        Json::Object(_) => "Hash",
    };
    err(format!("{} entry must be a map of a single instance path to its value, got {kind}", cf.upper_name()))
}

/// `cbor_seq2json`: decode a CBOR-sequence byte string into a sequence of
/// fetch/ipatch/post response entries.
pub fn cbor_seq_to_json(schema: &Schema, bytes: &[u8], cf: ContentFormat) -> R<Vec<Json>> {
    let items = cbor_seq_from_bytes(bytes)?;
    let mut out = Vec::with_capacity(items.len());
    for item in &items {
        match item {
            Cbor::Map(entries) if entries.len() == 1 => {
                let (iid, val) = &entries[0];
                let (node, path) = decode_iid(schema, iid)?;
                let decoded_val = if matches!(val, Cbor::Null) { Json::Null } else { decode_node_value(schema, node, val, cf)? };
                let mut obj = serde_json::Map::new();
                obj.insert(path, decoded_val);
                out.push(Json::Object(obj));
            }
            other if cf == ContentFormat::Fetch => {
                let (_, path) = decode_iid(schema, other)?;
                out.push(Json::String(path));
            }
            other => return Err(err(format!("unexpected top-level CBOR item {other:?} for {:?}", cf))),
        }
    }
    Ok(out)
}

/// `json2cbor`/whole-tree encode for `yang`/`get`/`put`.
pub fn json_to_cbor(schema: &Schema, value: &Json, cf: ContentFormat) -> R<Vec<u8>> {
    let obj = value.as_object().ok_or_else(|| err(format!("{:?} expects a JSON object at the top level, got {value}", cf)))?;
    let cbor = encode_body(schema, schema.root, obj, cf)?;
    cbor_to_bytes(&cbor)
}

/// `cbor2json`/whole-tree decode for `yang`/`get`/`put`.
pub fn cbor_to_json(schema: &Schema, bytes: &[u8], cf: ContentFormat) -> R<Json> {
    if bytes.is_empty() {
        return Ok(Json::Object(serde_json::Map::new()));
    }
    let items = cbor_seq_from_bytes(bytes)?;
    let value = items.into_iter().next().ok_or_else(|| err("empty CBOR payload"))?;
    decode_body(schema, schema.root, &value, cf)
}

// ===========================================================================
// base64 (RFC 4648), hand-rolled to avoid a dependency for this alone
// ===========================================================================

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = (b0 as u32) << 16 | (b1 as u32) << 8 | b2 as u32;
        out.push(B64_ALPHABET[(n >> 18 & 0x3F) as usize] as char);
        out.push(B64_ALPHABET[(n >> 12 & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 { B64_ALPHABET[(n >> 6 & 0x3F) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64_ALPHABET[(n & 0x3F) as usize] as char } else { '=' });
    }
    out
}

fn base64_decode(s: &str) -> R<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let clean: Vec<u8> = s.bytes().filter(|&b| b != b'=' && !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let vals: Vec<u32> = chunk.iter().map(|&c| val(c)).collect::<Option<Vec<_>>>().ok_or_else(|| err(format!("invalid base64 {s:?}")))?;
        let n = vals.iter().enumerate().fold(0u32, |acc, (i, &v)| acc | (v << (18 - 6 * i)));
        out.push((n >> 16) as u8);
        if vals.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if vals.len() > 3 {
            out.push(n as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", b"\x00\xff\x10"] {
            let enc = base64_encode(data);
            assert_eq!(base64_decode(&enc).unwrap(), data);
        }
    }

    #[test]
    fn format_ruby_float_matches_expected_quirk() {
        assert_eq!(format_ruby_float(257.0), "257.0");
        assert_eq!(format_ruby_float(0.0), "0.0");
        assert_eq!(format_ruby_float(25.7), "25.7");
    }
}
