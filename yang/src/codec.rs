// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! The SID-CBOR wire codec (RFC 9254 / RFC 9595), ported from
//! `support/yang-enc/yang-enc.rb`'s `type2cbor`/`type2json`/`json2cbor`/
//! `cbor2json`/`json_seq2cbor`/`cbor_seq2json`, cross-checked against
//! `sw-velocitydrive-devclient`'s `src/yang/yang-codec.ts` and
//! `client-lib/src/lm_yang.c`.

use cbor::Value as Cbor;
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

/// A builtin's own YANG-source spelling ("uint8", not `Builtin::Uint8`),
/// for error messages -- naming a *type* someone would recognize from
/// the YANG model, not this port's internal enum.
fn builtin_yang_name(b: Builtin) -> &'static str {
    match b {
        Builtin::Binary => "binary",
        Builtin::Bits => "bits",
        Builtin::Boolean => "boolean",
        Builtin::Decimal64 => "decimal64",
        Builtin::Empty => "empty",
        Builtin::Enumeration => "enumeration",
        Builtin::Identityref => "identityref",
        Builtin::InstanceIdentifier => "instance-identifier",
        Builtin::Int8 => "int8",
        Builtin::Int16 => "int16",
        Builtin::Int32 => "int32",
        Builtin::Int64 => "int64",
        Builtin::Uint8 => "uint8",
        Builtin::Uint16 => "uint16",
        Builtin::Uint32 => "uint32",
        Builtin::Uint64 => "uint64",
        Builtin::Leafref => "leafref",
        Builtin::String => "string",
        Builtin::Union => "union",
    }
}

/// The plain-YANG-name list of a union's member types, resolving past
/// any `leafref` member to what it actually targets -- "leafref" on its
/// own would tell a reader nothing about what value is actually wanted.
fn union_member_kinds(schema: &Schema, ty: &TypeDef) -> String {
    ty.union_members.iter().map(|&m| builtin_yang_name(resolved_builtin(schema, m).0)).collect::<Vec<_>>().join(", ")
}

/// Mirrors `print_error`'s two halves at once (yang-enc.rb:1234-1240):
/// on success, pass the value through; on failure, either propagate the
/// error (matches Ruby's `raise`) or warn to stderr and substitute
/// `fallback()` (matches Ruby's `STDERR.puts` followed by whatever the
/// call site does next -- skip a field, treat a shape mismatch as
/// empty, or return the raw un-encoded value from `type2cbor`'s own
/// rescue). `--continue`/`-c` is the only thing that ever makes
/// `continue_on_error` true.
fn lenient<T>(result: R<T>, continue_on_error: bool, fallback: impl FnOnce() -> T) -> R<T> {
    match result {
        Ok(v) => Ok(v),
        Err(e) if continue_on_error => {
            eprintln!("WARNING: {e} (continuing, --continue given)");
            Ok(fallback())
        }
        Err(e) => Err(e),
    }
}

/// A range/length bound as `i64`, narrowing `TypeDef::ranges`' `i128`
/// storage at the boundary. Every bound a real catalog actually
/// restricts fits `i64` exactly; the *default*, unrestricted bound
/// (`u64::MAX`, for string/binary length) doesn't, but clamping it here
/// is harmless -- it's not a real constraint to begin with (see
/// `schema.rs`'s `default_ranges`), so any value that could genuinely
/// arise still passes trivially.
fn range_bound_i64(v: i128) -> i64 {
    i64::try_from(v).unwrap_or(if v > 0 { i64::MAX } else { i64::MIN })
}

/// True if `ranges` is empty (nothing to restrict) or `value` falls in
/// at least one of its `(min, max)` pairs.
fn in_ranges(value: i64, ranges: &[(i128, i128)]) -> bool {
    ranges.is_empty() || ranges.iter().any(|&(min, max)| value >= range_bound_i64(min) && value <= range_bound_i64(max))
}

/// A plain-English description of a range/length restriction, for error
/// messages someone doesn't need to know YANG to act on -- "must be
/// between 0 and 255", not a type name or a raw range struct.
fn range_description(ranges: &[(i128, i128)]) -> String {
    let parts: Vec<String> = ranges
        .iter()
        .map(|&(min, max)| {
            let (min, max) = (range_bound_i64(min), range_bound_i64(max));
            if min == max { format!("exactly {min}") } else { format!("between {min} and {max}") }
        })
        .collect();
    parts.join(", or ")
}

fn matches_all_patterns(s: &str, patterns: &[String]) -> R<bool> {
    for p in patterns {
        let re = regex::Regex::new(p).map_err(|e| err(format!("invalid pattern {p:?} in schema: {e}")))?;
        if !re.is_match(s) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Encode a JSON value as CBOR for `type_id`. `in_union` tracks whether
/// this is (possibly nested) a member of a `union` -- enum/bits/
/// identityref/decimal64 wrap in a distinguishing CBOR tag only when
/// reached through a union (RFC 9254 6.3/6.6/6.7/6.10.1); otherwise they
/// use their plain (unwrapped) form since there's no ambiguity to
/// resolve.
///
/// This is also this port's *validator*: range/length/pattern checks
/// live right here, next to the type-shape checks they're a natural
/// extension of, rather than in a separately-generated JSON Schema
/// fed to a generic validator library (considered and deliberately not
/// done -- see this session's notes: it would need a real dependency
/// for what `TypeDef::ranges`/`patterns` already hold directly on the
/// node being encoded, and it can't help disagreeing with the encoder's
/// own identityref acceptance rule, since `type2schema`'s `enum`
/// deliberately excludes the base identity to match Ruby while the
/// encoder's own `all_identity_bases` correctly includes it per RFC
/// 7950 "derived from or equal to"). Any failure here -- shape,
/// range, length, pattern, or an unknown enum/bit/identity name (already
/// hard-checked by `encode_enum`/`encode_bits`/`encode_identityref`) --
/// gets the exact same `--continue` treatment via `lenient`, matching
/// `type2cbor`'s single `begin`/`rescue` wrapping its entire body
/// (yang-enc.rb:275-387).
pub fn type_to_cbor(schema: &Schema, type_id: TypeId, value: &Json, in_union: bool, continue_on_error: bool) -> R<Cbor> {
    lenient(type_to_cbor_checked(schema, type_id, value, in_union), continue_on_error, || json_to_cbor_generic(value))
}

fn type_to_cbor_checked(schema: &Schema, type_id: TypeId, value: &Json, in_union: bool) -> R<Cbor> {
    let ty = schema.ty(type_id);
    match ty.builtin {
        Builtin::Int8 | Builtin::Int16 | Builtin::Int32 | Builtin::Uint8 | Builtin::Uint16 | Builtin::Uint32 => {
            let n = json_number_to_i128(value).ok_or_else(|| err(format!("expected a whole number here, got {value}")))? as i64;
            if !in_ranges(n, &ty.ranges) {
                return Err(err(format!("{n} is not allowed here -- the value must be {}", range_description(&ty.ranges))));
            }
            Ok(Cbor::from(n))
        }
        Builtin::Int64 => {
            // RFC 7951 6.1: 64-bit integers are written as quoted text,
            // not a bare JSON number, so full precision survives any
            // JSON/JS-based tooling that only handles doubles safely.
            let s = value.as_str().ok_or_else(|| err(format!("expected a large whole number written as quoted text (e.g. \"12345678901\"), got {value}")))?;
            let n: i64 = s.parse().map_err(|_| err(format!("{s:?} is not a valid whole number")))?;
            Ok(Cbor::from(n))
        }
        Builtin::Uint64 => {
            let s = value.as_str().ok_or_else(|| err(format!("expected a large whole number written as quoted text (e.g. \"12345678901\"), got {value}")))?;
            let n: u64 = s.parse().map_err(|_| err(format!("{s:?} is not a valid non-negative whole number")))?;
            Ok(Cbor::from(n))
        }
        Builtin::Boolean => Ok(Cbor::Bool(value.as_bool().ok_or_else(|| err(format!("expected true or false, got {value}")))?)),
        Builtin::String => {
            let s = value.as_str().ok_or_else(|| err(format!("expected text here, got {value}")))?;
            let len = s.chars().count() as i64;
            if !in_ranges(len, &ty.ranges) {
                return Err(err(format!("{s:?} is {len} character(s) long, but must be {}", range_description(&ty.ranges))));
            }
            if !matches_all_patterns(s, &ty.patterns)? {
                return Err(err(format!("{s:?} is not in the expected format -- it must match: {}", ty.patterns.join(" and "))));
            }
            Ok(Cbor::Text(s.to_string()))
        }
        Builtin::Binary => {
            let s = value.as_str().ok_or_else(|| err(format!("expected base64-encoded text (for binary data), got {value}")))?;
            let bytes = base64_decode(s)?;
            let len = bytes.len() as i64;
            if !in_ranges(len, &ty.ranges) {
                return Err(err(format!("this value is {len} byte(s) long once decoded, but must be {}", range_description(&ty.ranges))));
            }
            Ok(Cbor::Bytes(bytes))
        }
        // RFC 7951 6.9: an `empty` leaf's JSON value is always a single-
        // element array containing `null` -- `to_json_schema`'s own
        // `{type:'array', items:{type:'null'}, minItems:1, maxItems:1}`
        // enforces exactly this shape (the union-dispatch heuristic
        // already checks it too, for picking a union member -- see
        // `pick_union_member_for_json`'s `Builtin::Empty` arm -- this is
        // that same check, now applied when `empty` isn't inside a
        // union either).
        Builtin::Empty => match value {
            Json::Array(a) if a.len() == 1 && a[0].is_null() => Ok(Cbor::Null),
            other => Err(err(format!(
                "this field has no value of its own -- it's just present or absent, so it must be written as [null] (a one-item list containing null), got {other}"
            ))),
        },
        Builtin::Decimal64 => encode_decimal64(ty, value, in_union),
        Builtin::Enumeration => encode_enum(ty, value, in_union),
        Builtin::Bits => encode_bits(ty, value, in_union),
        Builtin::Identityref => encode_identityref(schema, ty, value, in_union),
        Builtin::InstanceIdentifier => {
            let s = value.as_str().ok_or_else(|| err(format!("expected a path (as text) pointing to another node, got {value}")))?;
            resolve_iid(schema, s).map(|(_, cbor)| cbor)
        }
        Builtin::Leafref => match ty.leafref_target.and_then(|t| schema.node(t).type_id) {
            Some(target_type) => type_to_cbor_checked(schema, target_type, value, in_union),
            // Unresolved (e.g. a relative leafref, not exercised in this
            // catalog): fall back to passthrough so encoding still
            // succeeds rather than hard-failing.
            None => Ok(json_to_cbor_generic(value)),
        },
        Builtin::Union => {
            let member = pick_union_member_for_json(schema, ty, value)
                .ok_or_else(|| err(format!("{value} does not match any of the allowed types here: {}", union_member_kinds(schema, ty))))?;
            type_to_cbor_checked(schema, member, value, true)
        }
    }
}

/// A fully generic, infallible JSON->CBOR mapping, with no schema
/// involved at all -- mirrors `type2cbor`'s `return value` fallback
/// (yang-enc.rb:384-385): Ruby's dynamically-typed `value` can be
/// embedded directly into the surrounding CBOR structure regardless of
/// what the declared type actually was, since Ruby's CBOR encoder
/// serializes any native Hash/Array/String/Integer/... value as-is.
/// This is the Rust equivalent, recursing into arrays/objects so a
/// malformed *nested* value (not just a scalar) still gets *some*
/// representation on the wire under `--continue`.
fn json_to_cbor_generic(value: &Json) -> Cbor {
    match value {
        Json::Null => Cbor::Null,
        Json::Bool(b) => Cbor::Bool(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Cbor::from(i)
            } else if let Some(u) = n.as_u64() {
                Cbor::from(u)
            } else {
                // Not representable as a plain integer (e.g. a
                // fractional number) -- there's no CBOR float support
                // in this codebase at all (a real device can neither
                // send nor make sense of one), so fall back to the
                // number's own text form rather than a fabricated
                // approximation.
                Cbor::Text(n.to_string())
            }
        }
        Json::String(s) => Cbor::Text(s.clone()),
        Json::Array(items) => Cbor::Array(items.iter().map(json_to_cbor_generic).collect()),
        Json::Object(obj) => Cbor::Map(obj.iter().map(|(k, v)| (Cbor::Text(k.clone()), json_to_cbor_generic(v))).collect()),
    }
}

/// Decode CBOR back to a JSON value for `type_id`.
pub fn type_to_json(schema: &Schema, type_id: TypeId, value: &Cbor, in_union: bool) -> R<Json> {
    let ty = schema.ty(type_id);
    match ty.builtin {
        Builtin::Int8 | Builtin::Int16 | Builtin::Int32 | Builtin::Uint8 | Builtin::Uint16 | Builtin::Uint32 => {
            let i = cbor_as_i128(value).ok_or_else(|| unexpected_value("a whole number", value))?;
            Ok(Json::from(i as i64))
        }
        Builtin::Int64 => {
            let i = cbor_as_i128(value).ok_or_else(|| unexpected_value("a whole number", value))?;
            Ok(Json::String((i as i64).to_string()))
        }
        Builtin::Uint64 => {
            let i = cbor_as_i128(value).ok_or_else(|| unexpected_value("a whole number", value))?;
            Ok(Json::String((i as u64).to_string()))
        }
        Builtin::Boolean => Ok(Json::Bool(value.as_bool().ok_or_else(|| unexpected_value("true or false", value))?)),
        Builtin::String => Ok(Json::String(value.as_text().ok_or_else(|| unexpected_value("text", value))?.to_string())),
        Builtin::Binary => {
            let bytes = value.as_bytes().ok_or_else(|| unexpected_value("binary data", value))?;
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
            let member = pick_union_member_for_cbor(schema, ty, value).ok_or_else(|| {
                err(format!(
                    "the device sent {}, which doesn't match any of this field's allowed types ({}) -- the loaded YANG catalog may not match this device's firmware",
                    describe_cbor(value),
                    union_member_kinds(schema, ty)
                ))
            })?;
            type_to_json(schema, member, value, true)
        }
    }
}

fn cbor_scalar_passthrough_to_json(value: &Cbor) -> R<Json> {
    match value {
        Cbor::Text(s) => Ok(Json::String(s.clone())),
        Cbor::Bool(b) => Ok(Json::Bool(*b)),
        Cbor::Integer(_) => Ok(Json::from(cbor_as_i128(value).unwrap_or(0) as i64)),
        Cbor::Null => Ok(Json::Null),
        other => Err(err(format!("the device sent {}, which this port has no defined way to decode here", describe_cbor(other)))),
    }
}

fn cbor_as_i128(value: &Cbor) -> Option<i128> {
    match value {
        Cbor::Integer(i) => Some(*i),
        _ => None,
    }
}

/// A short, plain-English description of a CBOR value for error
/// messages, in place of ciborium's own `Debug` output (e.g.
/// `Integer(Integer(300))`) -- decode errors are diagnostic (a device
/// sent something this port's schema couldn't make sense of, not
/// something the reader typed), so what matters is naming the *kind* of
/// value seen without leaking this port's own library internals.
/// A decode-side type mismatch: the device's response doesn't match
/// what this leaf's YANG type says to expect. Framed as coming from the
/// device, not something the reader typed -- there's nothing to "fix"
/// in a request here, only a hint that the loaded YANG catalog may not
/// match this device's firmware.
fn unexpected_value(expected: &str, value: &Cbor) -> CodecError {
    err(format!("expected {expected} in the device's response, but got {} -- the loaded YANG catalog may not match this device's firmware", describe_cbor(value)))
}

fn describe_cbor(value: &Cbor) -> String {
    match value {
        Cbor::Integer(_) => format!("the number {}", cbor_as_i128(value).unwrap_or_default()),
        Cbor::Text(s) => format!("the text {s:?}"),
        Cbor::Bytes(b) => format!("{} byte(s) of binary data", b.len()),
        Cbor::Bool(b) => format!("the boolean {b}"),
        Cbor::Null => "null".to_string(),
        Cbor::Array(items) => format!("a list of {} item(s)", items.len()),
        Cbor::Map(entries) => format!("a map of {} entrie(s)", entries.len()),
        Cbor::Tag(t, inner) => format!("a tagged value (tag {t}, containing {})", describe_cbor(inner)),
    }
}

// -- decimal64 ---------------------------------------------------------

fn encode_decimal64(ty: &TypeDef, value: &Json, _in_union: bool) -> R<Cbor> {
    let fraction_digits = ty.fraction_digits.ok_or_else(|| err("this decimal field is missing its fraction-digits setting in the schema -- this looks like a catalog problem"))? as usize;
    let s = value.as_str().ok_or_else(|| err(format!("expected a decimal number written as text (e.g. \"12.34\"), got {value}")))?;
    let (sign, rest) = match s.strip_prefix('-') {
        Some(r) => (-1i128, r),
        None => (1i128, s),
    };
    let (int_part, frac_part) = rest.split_once('.').unwrap_or((rest, ""));
    let int_part = if int_part.is_empty() { "0" } else { int_part };
    if frac_part.len() > fraction_digits {
        return Err(err(format!("{s:?} has more decimal places than allowed here (at most {fraction_digits})")));
    }
    let padded_frac = format!("{frac_part:0<fraction_digits$}");
    let mantissa: i128 = format!("{int_part}{padded_frac}").parse().map_err(|_| err(format!("{s:?} is not a valid decimal number")))?;
    let mantissa = sign * mantissa;
    // decimal64 is always tag(4)-wrapped per RFC 9254 6.3, union or not
    // (unlike enum/bits/identityref, which only tag-wrap inside a union).
    Ok(Cbor::Tag(4, Box::new(Cbor::Array(vec![Cbor::from(-(fraction_digits as i64)), Cbor::from(mantissa as i64)]))))
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
                let exp = cbor_as_i128(&arr[0]).ok_or_else(|| err("the device sent a decimal number whose exponent isn't a valid integer"))? as i64;
                let mantissa = cbor_as_i128(&arr[1]).ok_or_else(|| err("the device sent a decimal number whose value isn't a valid integer"))?;
                Ok((exp, mantissa))
            }
            _ => Err(err(format!("the device sent a malformed decimal number ({})", describe_cbor(inner)))),
        },
        other => Err(unexpected_value("a decimal number", other)),
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
    let name = value.as_str().ok_or_else(|| err(format!("expected one of this field's allowed text values, got {value}")))?;
    let e = ty.enums.iter().find(|e| e.name == name).ok_or_else(|| err(format!("{name:?} is not one of the allowed values for this field")))?;
    if in_union {
        Ok(Cbor::Tag(44, Box::new(Cbor::Text(name.to_string()))))
    } else {
        Ok(Cbor::from(e.value))
    }
}

fn decode_enum(ty: &TypeDef, value: &Cbor, in_union: bool) -> R<Json> {
    if in_union {
        if let Cbor::Tag(44, inner) = value {
            let name = inner.as_text().ok_or_else(|| unexpected_value("text (an enum value)", inner))?;
            return Ok(Json::String(name.to_string()));
        }
    }
    let n = cbor_as_i128(value).ok_or_else(|| unexpected_value("a whole number (an enum value)", value))?;
    match ty.enums.iter().find(|e| e.value as i128 == n) {
        Some(e) => Ok(Json::String(e.name.clone())),
        None => {
            eprintln!("WARNING: the device reported enum value {n}, which isn't defined in the loaded YANG catalog (the device's firmware may be newer than the catalog) -- showing the raw number instead of a name");
            Ok(Json::from(n as i64))
        }
    }
}

// -- bits (RFC 9254 6.7) -------------------------------------------------

fn encode_bits(ty: &TypeDef, value: &Json, in_union: bool) -> R<Cbor> {
    let names: Vec<&str> = match value {
        Json::String(s) => s.split_whitespace().collect(),
        Json::Array(items) => items.iter().filter_map(|v| v.as_str()).collect(),
        other => return Err(err(format!("expected a space-separated list of names (as text, or a list of names), got {other}"))),
    };
    if in_union {
        return Ok(Cbor::Tag(43, Box::new(Cbor::Text(names.join(" ")))));
    }

    let mut positions = Vec::new();
    for name in &names {
        let bit = ty.bits.iter().find(|b| b.name == *name).ok_or_else(|| err(format!("{name:?} is not one of the allowed names for this field")))?;
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
        if pending_gap > 0 {
            out.push(Cbor::from(pending_gap));
            pending_gap = 0;
        }
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
            let s = inner.as_text().ok_or_else(|| unexpected_value("text (a list of names)", inner))?;
            return Ok(Json::String(s.to_string()));
        }
    }

    let spans: Vec<&Cbor> = match value {
        Cbor::Bytes(_) => vec![value],
        Cbor::Array(items) => items.iter().collect(),
        other => return Err(unexpected_value("bit-flag data", other)),
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
            other => return Err(err(format!("the device sent malformed bit-flag data (an unexpected {})", describe_cbor(other)))),
        }
    }
    Ok(Json::String(names.join(" ")))
}

// -- identityref (RFC 9254 6.10 / RFC 9595) ------------------------------

pub(crate) fn all_identity_bases(schema: &Schema, ty: &TypeDef) -> Option<std::collections::HashSet<IdentityId>> {
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
    let s = value.as_str().ok_or_else(|| err(format!("expected the name of one of this field's allowed values (as text), got {value}")))?;
    let candidates = all_identity_bases(schema, ty).ok_or_else(|| err("this field's schema doesn't declare any allowed values at all -- this looks like a catalog problem"))?;
    let source_module = ty.source_module.as_deref().unwrap_or("");

    let found = candidates.into_iter().find(|&id| {
        let identity = schema.identity(id);
        match s.split_once(':') {
            Some((m, n)) => identity.module == m && identity.name == n,
            None => identity.module == source_module && identity.name == s,
        }
    });
    let id = found.ok_or_else(|| err(format!("{s:?} is not one of the allowed values for this field")))?;
    let sid = schema.identity(id).sid.ok_or_else(|| err(format!("{s:?} exists in the schema but has no SID assigned -- this looks like a catalog problem")))?;
    if in_union {
        Ok(Cbor::Tag(45, Box::new(Cbor::from(sid))))
    } else {
        Ok(Cbor::from(sid))
    }
}

fn decode_identityref(schema: &Schema, _ty: &TypeDef, value: &Cbor, in_union: bool) -> R<Json> {
    let sid = if in_union {
        match value {
            Cbor::Tag(45, inner) => cbor_as_i128(inner).ok_or_else(|| unexpected_value("a whole number (an identity reference)", inner))?,
            other => return Err(unexpected_value("an identity reference (tag 45)", other)),
        }
    } else {
        cbor_as_i128(value).ok_or_else(|| unexpected_value("a whole number (an identity reference)", value))?
    };
    let identity = schema
        .identities
        .iter()
        .find(|i| i.sid == Some(sid as i64))
        .ok_or_else(|| err(format!("the device referenced identity #{sid}, which isn't in the loaded YANG catalog -- the catalog may not match this device's firmware")))?;
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
            (Builtin::Empty, Json::Array(a)) if a.len() == 1 && a[0].is_null() => return Some(m),
            _ => {}
        }
    }
    // String preferred over Binary/InstanceIdentifier when a plain JSON
    // string could structurally match any of them (an empty string in
    // particular is ambiguous -- base64-decodes trivially to empty bytes
    // -- so pick the more general/common member first, in separate
    // passes, rather than whichever happens to be declared first).
    if let Some(&m) = members.iter().find(|&&m| resolved_builtin(schema, m).0 == Builtin::String) {
        if matches!(value, Json::String(_)) {
            return Some(m);
        }
    }
    for &m in &members {
        if matches!(resolved_builtin(schema, m).0, Builtin::Binary | Builtin::InstanceIdentifier) && matches!(value, Json::String(_)) {
            return Some(m);
        }
    }
    // No heuristic above matched -- genuinely ambiguous/unmatched, not
    // "pick something and hope": returning the first member
    // unconditionally here (an earlier version of this did) would make
    // `type_to_cbor_checked`'s "no member matches" error effectively
    // dead code, and would report failure against an arbitrary member
    // instead of saying plainly that nothing in the union fits.
    None
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
    // No member's builtin kind matches this CBOR value's own kind --
    // genuinely unmatched, not "pick something and hope" (see
    // `pick_union_member_for_json`'s matching comment): blindly falling
    // back to the first member here would decode against the wrong
    // type and report a confusing error unrelated to the real mismatch.
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
        let close = r[open..].find(']').ok_or_else(|| err(format!("the path segment {seg:?} has an opening '[' with no matching ']'")))?;
        let inner = &r[open + 1..open + close];
        let (k, v) = inner.split_once('=').ok_or_else(|| err(format!("{inner:?} in {seg:?} isn't a key selector -- it should look like key='value'")))?;
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
            let n: i64 = raw.parse().map_err(|_| err(format!("{raw:?} is not a whole number, but this list key needs one")))?;
            Ok(Json::from(n))
        }
        Builtin::Uint8 | Builtin::Uint16 | Builtin::Uint32 => {
            let n: u64 = raw.parse().map_err(|_| err(format!("{raw:?} is not a whole number, but this list key needs one")))?;
            Ok(Json::from(n))
        }
        Builtin::Boolean => match raw {
            "true" => Ok(Json::Bool(true)),
            "false" => Ok(Json::Bool(false)),
            other => Err(err(format!("{other:?} is not \"true\" or \"false\", but this list key needs one of those"))),
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
        let child = schema
            .find_child(cur, seg.name)
            .ok_or_else(|| err(format!("\"{}\" in the path {path:?} doesn't exist in this device's YANG model -- check for a typo", seg.name)))?;
        let node = schema.node(child);
        let sid = node
            .sid
            .ok_or_else(|| err(format!("\"{}\" exists in the schema but has no SID assigned -- this looks like a catalog problem, not a mistake in your path", seg.name)))?;

        let mut ordered_keys = Vec::new();
        if !node.keys.is_empty() {
            for key_name in &node.keys {
                if let Some((_, v)) = seg.keys.iter().find(|(k, _)| k == key_name) {
                    let key_child = schema
                        .find_child(child, key_name)
                        .ok_or_else(|| err(format!("the key \"{key_name}\" of \"{}\" doesn't exist in this device's YANG model ({path:?})", seg.name)))?;
                    let key_type = schema
                        .node(key_child)
                        .type_id
                        .ok_or_else(|| err(format!("the key \"{key_name}\" has no type in the schema -- this looks like a catalog problem")))?;
                    let json_v = convert_iid_key_value(schema, key_type, v)?;
                    // Matches Ruby's `iid2cbor` (yang-enc.rb:459-467): its
                    // own `type2cbor` call for a key value never receives
                    // `continue_on_error` at all (implicit `false`) --
                    // instance-identifier resolution is always strict,
                    // regardless of `--continue`.
                    let cbor_v = type_to_cbor(schema, key_type, &json_v, false, false)?;
                    ordered_keys.push(cbor_v);
                }
            }
            for (k, _) in &seg.keys {
                if !node.keys.iter().any(|nk| nk == k) {
                    return Err(err(format!("\"{k}\" is not a key of the list at {path:?} -- check the key name")));
                }
            }
        } else if !seg.keys.is_empty() {
            return Err(err(format!("\"{}\" is not a list, so it can't take key='value' selectors like in {path:?}", seg.name)));
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
        Cbor::Integer(_) => Ok((cbor_as_i128(value).ok_or_else(|| err("the device sent a path identifier that isn't a valid integer"))? as i64, Vec::new())),
        Cbor::Array(items) => {
            let mut it = items.iter();
            let sid = it
                .next()
                .and_then(cbor_as_i128)
                .ok_or_else(|| err(format!("the device sent a path identifier (a {}-item list) with no valid number in the first position", items.len())))?
                as i64;
            Ok((sid, it.cloned().collect()))
        }
        other => Err(err(format!(
            "the device sent a path identifier that's neither a plain number nor a list starting with one -- got {}",
            describe_cbor(other)
        ))),
    }
}

/// Decode a CBOR IID back to `find_node_from_sid`'s node plus a rebuilt
/// path string (single-quoted key values, canonical form -- not
/// necessarily byte-identical to whatever original string produced the
/// SID, matching `iid2json`'s behavior).
pub fn decode_iid(schema: &Schema, value: &Cbor) -> R<(NodeId, String)> {
    let (sid, mut keys) = split_iid_cbor(value)?;
    let node = *schema
        .sid_index
        .get(&sid)
        .ok_or_else(|| err(format!("the device referenced path identifier #{sid}, which isn't in the loaded YANG catalog -- the catalog may not match this device's firmware")))?;
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
                let key_child = schema
                    .find_child(n, key_name)
                    .ok_or_else(|| err(format!("the list \"{}\" is missing its own key field \"{key_name}\" in the schema -- this looks like a catalog problem", nd.name)))?;
                let key_type = schema
                    .node(key_child)
                    .type_id
                    .ok_or_else(|| err(format!("the key \"{key_name}\" has no type in the schema -- this looks like a catalog problem")))?;
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

/// A name for `node` fit for an error message: the synthetic multi-
/// module root (`schema.rs`'s `finish()`) is called `"data-tree-schema"`
/// internally, which means nothing to someone who didn't write this
/// port -- call it what it is to them instead.
fn node_display_name(node: &Node) -> &str {
    if node.name == "data-tree-schema" {
        "the request"
    } else {
        &node.name
    }
}

fn effective_delta_base(schema: &Schema, node: NodeId) -> i64 {
    let n = schema.node(node);
    if n.kw == "input" || n.kw == "output" {
        if let Some(parent) = n.parent {
            if IMPLICIT_INPUT_OUTPUT_BASE.contains(&schema.node(parent).kw.as_str()) {
                if let Some(sid) = schema.node(parent).sid {
                    return sid;
                }
            }
        }
    }
    n.sid.unwrap_or(0)
}

fn encode_leaf_value(schema: &Schema, node: &Node, value: &Json, continue_on_error: bool) -> R<Cbor> {
    let type_id = node.type_id.ok_or_else(|| err(format!("\"{}\" has no type defined in the schema -- this looks like a catalog problem", node.name)))?;
    type_to_cbor(schema, type_id, value, false, continue_on_error)
}

fn decode_leaf_value(schema: &Schema, node: &Node, value: &Cbor) -> R<Json> {
    let type_id = node.type_id.ok_or_else(|| err(format!("\"{}\" has no type defined in the schema -- this looks like a catalog problem", node.name)))?;
    type_to_json(schema, type_id, value, false)
}

/// Encode one container's (or one list entry's, or one rpc input/output's)
/// body: a JSON object of child-name -> value into a CBOR map keyed by
/// delta-SID.
///
/// Two error classes here are `--continue`-gated, mirroring
/// `json2cbor_hash` (yang-enc.rb:176-191) exactly: an input key with no
/// matching schema child, or a matching child with no SID, skips that
/// one field (via `lenient`) instead of failing the whole body. The
/// mandatory-field/list-key check below is *not* in `json2cbor_hash` at
/// all -- Ruby's `json_schemer`-based pre-check catches a missing
/// required field before encoding ever starts, a step this port doesn't
/// have (see `type_to_cbor`'s doc comment) -- so it's added here
/// instead, gated the same way for consistency with everything else
/// `--continue` softens.
fn encode_body(schema: &Schema, scope: NodeId, obj: &serde_json::Map<String, Json>, cf: ContentFormat, continue_on_error: bool) -> R<Cbor> {
    let base = effective_delta_base(schema, scope);
    let scope_node = schema.node(scope);
    // Matches `json2cbor_hash` (support/yang-enc/yang-enc.rb:176-191)
    // exactly: map entries come out in the *input* JSON/YAML object's own
    // key order, full stop -- not schema declaration order, not ascending
    // delta-SID, and (unlike `resolve_iid`'s instance-identifier key
    // array) list-entry fields are not reordered to key-statement order
    // either. `serde_json::Map` is IndexMap-backed here (the
    // "preserve_order" feature), so iterating `obj` directly already
    // preserves that order. Matched against the child's full `name`
    // (already qualified "module:local" for a node augmented in from a
    // different module, bare otherwise; RFC 7951 4.2's JSON-member-name
    // rule).
    let mut entries = Vec::new();
    for (key, val) in obj {
        let found = schema
            .find_child(scope, key)
            .ok_or_else(|| err(format!("{key:?} is not a valid field inside {} -- check for a typo or the wrong nesting", node_display_name(scope_node))))
            .and_then(|child| {
                schema
                    .node(child)
                    .sid
                    .map(|sid| (child, sid))
                    .ok_or_else(|| err(format!("{key:?} exists in the schema but has no SID assigned -- this looks like a catalog/schema problem, not a mistake in your data")))
            });
        let Some((child, child_sid)) = lenient(found.map(Some), continue_on_error, || None)? else { continue };
        let cbor_val = encode_node_value(schema, child, val, cf, continue_on_error)?;
        entries.push((Cbor::from(child_sid - base), cbor_val));
    }

    // Anything required (a mandatory child, or -- when this scope is a
    // `list` node -- one of its own keys) but absent from `obj`.
    // Operational-state (`config false`) children are never required for
    // an ipatch/put request, matching `to_json_schema`'s own filter.
    for &child in &scope_node.children {
        let c = schema.node(child);
        if matches!(cf, ContentFormat::Ipatch | ContentFormat::Put) && !c.config {
            continue;
        }
        let is_key = scope_node.keys.iter().any(|k| k == c.local_name());
        if (c.mandatory || is_key) && !obj.contains_key(&c.name) {
            let why = if is_key { "it's a key of this list -- every entry needs one" } else { "it's a required field" };
            lenient(Err(err(format!("{:?} is missing from {} ({why})", c.name, node_display_name(scope_node)))), continue_on_error, || ())?;
        }
    }

    Ok(Cbor::Map(entries))
}

fn decode_body(schema: &Schema, scope: NodeId, value: &Cbor, cf: ContentFormat) -> R<Json> {
    let base = effective_delta_base(schema, scope);
    let map = value
        .as_map()
        .ok_or_else(|| err(format!("expected a set of fields for {}, but the device sent {}", node_display_name(schema.node(scope)), describe_cbor(value))))?;
    let mut obj = serde_json::Map::new();
    for (k, v) in map {
        let delta = cbor_as_i128(k).ok_or_else(|| err("the device sent a field whose identifier isn't a valid number"))? as i64;
        let child = *schema.sid_index.get(&(delta + base)).ok_or_else(|| {
            err(format!(
                "the device sent field #{}, which isn't in the loaded YANG catalog -- the catalog may not match this device's firmware",
                delta + base
            ))
        })?;
        let name = schema.node(child).name.clone();
        obj.insert(name, decode_node_value(schema, child, v, cf)?);
    }
    Ok(Json::Object(obj))
}

/// Encode a single JSON value for `node` (whatever kind it is) into CBOR.
///
/// The container/list shape checks below are `--continue`-gated
/// (falling back to an empty `{}`/`[]`, matching Ruby's own
/// `result = {}`/`result = []` defaults), mirroring `json2cbor`'s
/// `'module'`/`'container'`/`'input'`/`'output'`/`'list'` branches
/// (yang-enc.rb:212-241) exactly. `leaf-list` and `rpc`/`action`'s own
/// top-level shape assumption have no such guard in Ruby either (an
/// unguarded `json.map`/`.keys` call that would just crash on the wrong
/// shape) -- replicated here as unconditional hard errors, not a
/// missing feature.
pub fn encode_node_value(schema: &Schema, node: NodeId, value: &Json, cf: ContentFormat, continue_on_error: bool) -> R<Cbor> {
    let n = schema.node(node);
    match n.kw.as_str() {
        "leaf" => encode_leaf_value(schema, n, value, continue_on_error),
        "leaf-list" => {
            let items = value.as_array().ok_or_else(|| err(format!("\"{}\" needs a list of values, got {value}", node_display_name(n))))?;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(encode_leaf_value(schema, n, item, continue_on_error)?);
            }
            Ok(Cbor::Array(out))
        }
        "container" | "input" | "output" => lenient(
            value
                .as_object()
                .ok_or_else(|| err(format!("\"{}\" needs an object (key/value fields), got {value}", node_display_name(n))))
                .and_then(|obj| encode_body(schema, node, obj, cf, continue_on_error)),
            continue_on_error,
            || Cbor::Map(vec![]),
        ),
        "list" => lenient(
            match value {
                Json::Array(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        let obj = item.as_object().ok_or_else(|| err(format!("each entry in \"{}\" needs to be an object (key/value fields), got {item}", node_display_name(n))))?;
                        out.push(encode_body(schema, node, obj, cf, continue_on_error)?);
                    }
                    Ok(Cbor::Array(out))
                }
                Json::Object(obj) if cf.is_sequence() => encode_body(schema, node, obj, cf, continue_on_error),
                other => Err(err(format!(
                    "\"{}\" needs a list{}, got {other}",
                    node_display_name(n),
                    if cf.is_sequence() { " (or a single entry, for fetch/ipatch)" } else { "" }
                ))),
            },
            continue_on_error,
            || Cbor::Array(vec![]),
        ),
        "rpc" | "action" => {
            // On the wire this is always flat (the input/output params
            // directly, keyed by delta from the *rpc's* own SID -- see
            // `effective_delta_base`). As user-facing YAML/JSON input,
            // though, a single `input:`/`output:` wrapper key disambiguates
            // which side is meant; unwrap it if present.
            let obj = value.as_object().ok_or_else(|| err(format!("\"{}\" needs an object (its input/output fields), got {value}", node_display_name(n))))?;
            if obj.len() == 1 {
                for side_name in ["input", "output"] {
                    if let Some(inner) = obj.get(side_name) {
                        let side_node = schema
                            .find_child(node, side_name)
                            .ok_or_else(|| err(format!("\"{}\" doesn't have a \"{side_name}\" side in the schema -- this looks like a catalog problem", node_display_name(n))))?;
                        let inner_obj = inner
                            .as_object()
                            .ok_or_else(|| err(format!("the \"{side_name}\" of \"{}\" needs an object (key/value fields), got {inner}", node_display_name(n))))?;
                        return encode_body(schema, side_node, inner_obj, cf, continue_on_error);
                    }
                }
            }
            let side = schema.find_child(node, "input").unwrap_or(node);
            encode_body(schema, side, obj, cf, continue_on_error)
        }
        other => Err(err(format!("\"{}\" has an unsupported schema shape ({other:?}) -- this looks like a catalog problem", node_display_name(n)))),
    }
}

pub fn decode_node_value(schema: &Schema, node: NodeId, value: &Cbor, cf: ContentFormat) -> R<Json> {
    let n = schema.node(node);
    match n.kw.as_str() {
        "leaf" => decode_leaf_value(schema, n, value),
        "leaf-list" => {
            let items = value.as_array().ok_or_else(|| err(format!("\"{}\" needs a list of values, but the device sent {}", node_display_name(n), describe_cbor(value))))?;
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
            other => Err(err(format!("\"{}\" needs a list, but the device sent {}", node_display_name(n), describe_cbor(other)))),
        },
        "rpc" | "action" => {
            // Mirrors `cbor2json`'s `'rpc'`/`'action'` branch
            // (yang-enc.rb:757-794). The wire payload is always flat (no
            // `input`/`output` wrapper -- see `encode_node_value`'s
            // matching comment): both sides' children are deltas from
            // the *rpc's own* SID (`effective_delta_base`), so which
            // side a payload belongs to isn't recoverable from any one
            // key in isolation via the global `sid_index` (that lookup
            // would resolve to a real node either way, whichever side it
            // actually belongs to -- an earlier version of this tried
            // "decode as output, fall back to input on error" and that
            // never actually failed, always spuriously picking output).
            // Ruby's own algorithm -- replicated here -- looks at the
            // *first* key only, and checks each side's *direct* children
            // (not a global lookup) for one whose SID matches; input is
            // checked before output, same as Ruby's substatement order.
            let map = value
                .as_map()
                .ok_or_else(|| err(format!("\"{}\" needs an object (its input/output fields), but the device sent {}", node_display_name(n), describe_cbor(value))))?;
            if map.is_empty() {
                // No mandatory parameters on this side -- ok, matches
                // Ruby's `cbor.is_a? Hash and cbor.empty?` case.
                return Ok(Json::Object(serde_json::Map::new()));
            }
            let base = n.sid.unwrap_or(0);
            let (first_key, _) = map.first().expect("checked non-empty above");
            let delta = cbor_as_i128(first_key).ok_or_else(|| err("the device sent a field whose identifier isn't a valid number"))? as i64;
            let absolute_sid = delta + base;
            let side_name = ["input", "output"]
                .into_iter()
                .find(|&side| {
                    schema
                        .find_child(node, side)
                        .is_some_and(|side_node| schema.node(side_node).children.iter().any(|&c| schema.node(c).sid == Some(absolute_sid)))
                })
                .ok_or_else(|| {
                    err(format!(
                        "couldn't tell whether \"{}\"'s response is its input or output parameters (field #{absolute_sid} matches neither) -- the loaded YANG catalog may not match this device's firmware",
                        node_display_name(n)
                    ))
                })?;
            let side_node = schema.find_child(node, side_name).expect("just matched above");
            let decoded = decode_body(schema, side_node, value, cf)?;
            let mut wrapper = serde_json::Map::new();
            wrapper.insert(side_name.to_string(), decoded);
            Ok(Json::Object(wrapper))
        }
        other => Err(err(format!("\"{}\" has an unsupported schema shape ({other:?}) -- this looks like a catalog problem", node_display_name(n)))),
    }
}

// ===========================================================================
// Top-level entry points
// ===========================================================================

fn cbor_to_bytes(value: &Cbor) -> R<Vec<u8>> {
    Ok(cbor::to_vec(value))
}

fn cbor_seq_from_bytes(bytes: &[u8]) -> R<Vec<Cbor>> {
    let mut pos = 0;
    let mut items = Vec::new();
    while pos < bytes.len() {
        let v = cbor::from_slice(bytes, &mut pos).map_err(|e| err(format!("could not decode this as CBOR ({e}) -- the data may be corrupted, truncated, or not CBOR at all")))?;
        items.push(v);
    }
    Ok(items)
}

/// `fercc conv`'s `['cbor', 'cbor']` case (yang-enc.rb:164-167): decode a
/// CBOR-sequence byte string generically (no schema, no content-format
/// -- each top-level item as whatever CBOR value it is) and re-encode
/// each item individually. A normalize pass, not a byte-identical
/// passthrough: canonicalizes indefinite-length items, non-minimal
/// integer encodings, etc.
pub fn normalize_cbor_seq(bytes: &[u8]) -> R<Vec<u8>> {
    let items = cbor_seq_from_bytes(bytes)?;
    let mut out = Vec::new();
    for item in &items {
        out.extend(cbor_to_bytes(item)?);
    }
    Ok(out)
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
pub fn json_seq_to_cbor(schema: &Schema, items: &[Json], cf: ContentFormat, continue_on_error: bool) -> R<Vec<u8>> {
    let mut out = Vec::new();
    for item in items {
        let cbor_item = match cf {
            ContentFormat::Fetch => match item {
                Json::String(path) => resolve_iid(schema, path)?.1,
                Json::Object(map) if map.len() == 1 => {
                    let (path, val) = map.iter().next().unwrap();
                    let (target, iid) = resolve_iid(schema, path)?;
                    let encoded_val = if val.is_null() { Cbor::Null } else { encode_node_value(schema, target, val, cf, continue_on_error)? };
                    Cbor::Map(vec![(iid, encoded_val)])
                }
                // Wrong key *count* (0 or >1) gets the specific
                // "exactly one key/value pair" message, not the generic
                // shape-mismatch one below -- matches Ipatch/Post's
                // `validate_instance_entry` for the same case.
                Json::Object(_) => {
                    validate_single_key(cf, item)?;
                    unreachable!("validate_single_key only returns Ok for a single-key map, already handled above")
                }
                other => return Err(validate_entry_err(cf, other)),
            },
            ContentFormat::Ipatch | ContentFormat::Post => {
                validate_instance_entry(cf, item)?;
                let map = item.as_object().unwrap();
                let (path, val) = map.iter().next().unwrap();
                let (target, iid) = resolve_iid(schema, path)?;
                let encoded_val = if val.is_null() { Cbor::Null } else { encode_node_value(schema, target, val, cf, continue_on_error)? };
                Cbor::Map(vec![(iid, encoded_val)])
            }
            ContentFormat::Yang | ContentFormat::Get | ContentFormat::Put => {
                return Err(err(format!("internal error: {} doesn't use sequence encoding and shouldn't reach this code path", cf.upper_name())));
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
            other => return Err(err(format!("the device sent {} as a {} response entry, which doesn't fit the expected shape", describe_cbor(other), cf.upper_name()))),
        }
    }
    Ok(out)
}

/// `json2cbor`/whole-tree encode for `yang`/`get`/`put`. The top-level
/// shape check is `--continue`-gated (falls back to an empty map),
/// matching `json2cbor`'s `'module'` branch (yang-enc.rb:212-222) --
/// `schema.root`'s own `kw` is `"module"`.
pub fn json_to_cbor(schema: &Schema, value: &Json, cf: ContentFormat, continue_on_error: bool) -> R<Vec<u8>> {
    let cbor = lenient(
        value
            .as_object()
            .ok_or_else(|| err(format!("a {} request needs an object (key/value fields) at the top level, got {value}", cf.upper_name())))
            .and_then(|obj| encode_body(schema, schema.root, obj, cf, continue_on_error)),
        continue_on_error,
        || Cbor::Map(vec![]),
    )?;
    cbor_to_bytes(&cbor)
}

/// `cbor2json`/whole-tree decode for `yang`/`get`/`put`.
pub fn cbor_to_json(schema: &Schema, bytes: &[u8], cf: ContentFormat) -> R<Json> {
    if bytes.is_empty() {
        return Ok(Json::Object(serde_json::Map::new()));
    }
    let items = cbor_seq_from_bytes(bytes)?;
    let value = items.into_iter().next().ok_or_else(|| err("could not read any CBOR value from this response"))?;
    decode_body(schema, schema.root, &value, cf)
}

// ===========================================================================
// base64 (RFC 4648), hand-rolled to avoid a dependency for this alone
// ===========================================================================

const B64_ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
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
        let vals: Vec<u32> = chunk.iter().map(|&c| val(c)).collect::<Option<Vec<_>>>().ok_or_else(|| err(format!("{s:?} is not valid base64-encoded text")))?;
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
    use crate::schema::Bit;

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

    // Ported from support/yang-enc/spec/instance_entry_spec.rb: exact
    // error-message wording for a malformed fetch/ipatch/post entry.
    // These are validation-only checks (fail before any schema/SID
    // lookup), so an empty schema is enough -- and they cover the error
    // paths the success-oriented devclient fixture corpus never
    // exercises (see yang/tests/fixtures.rs).

    fn empty_schema() -> Schema {
        crate::schema::build(&[], &[]).expect("empty schema builds")
    }

    #[test]
    fn rejects_a_non_map_array_entry() {
        let schema = empty_schema();
        let err = json_seq_to_cbor(&schema, &[Json::Array(vec![Json::String("/some/path".into())])], ContentFormat::Ipatch, false).unwrap_err();
        assert!(err.0.contains("IPATCH"), "{}", err.0);
        assert!(err.0.contains("got array"), "{}", err.0);
    }

    #[test]
    fn rejects_a_scalar_entry() {
        let schema = empty_schema();
        let err = json_seq_to_cbor(&schema, &[Json::String("/some/path".into())], ContentFormat::Post, false).unwrap_err();
        assert!(err.0.contains("POST"), "{}", err.0);
        assert!(err.0.contains("got String"), "{}", err.0);
    }

    #[test]
    fn rejects_a_null_entry() {
        let schema = empty_schema();
        let err = json_seq_to_cbor(&schema, &[Json::Null], ContentFormat::Ipatch, false).unwrap_err();
        assert!(err.0.contains("got null"), "{}", err.0);
    }

    #[test]
    fn rejects_an_entry_with_two_keys_listing_them() {
        let schema = empty_schema();
        let mut map = serde_json::Map::new();
        map.insert("/a".to_string(), Json::from(1));
        map.insert("/b".to_string(), Json::from(2));
        let err = json_seq_to_cbor(&schema, &[Json::Object(map)], ContentFormat::Ipatch, false).unwrap_err();
        assert!(err.0.contains("exactly one key/value pair"), "{}", err.0);
        assert!(err.0.contains("got 2"), "{}", err.0);
        assert!(err.0.contains("/a") && err.0.contains("/b"), "{}", err.0);
    }

    #[test]
    fn rejects_an_empty_map() {
        let schema = empty_schema();
        let err = json_seq_to_cbor(&schema, &[Json::Object(serde_json::Map::new())], ContentFormat::Fetch, false).unwrap_err();
        assert!(err.0.contains("got 0"), "{}", err.0);
    }

    // Ported from yang-enc_spec.rb's `type2cbor`/`type2json` bits
    // examples: exact RFC 9254 6.7 sparse/dense byte patterns, pinned
    // independently of any real catalog's bit layout.

    fn alarm_state_type() -> TypeDef {
        let mut ty = TypeDef::new(Builtin::Bits);
        ty.bits = vec![
            Bit { name: "unknown".into(), position: 0 },
            Bit { name: "under-repair".into(), position: 1 },
            Bit { name: "critical".into(), position: 2 },
            Bit { name: "major".into(), position: 3 },
            Bit { name: "minor".into(), position: 4 },
            Bit { name: "warning".into(), position: 8 },
            Bit { name: "indeterminate".into(), position: 128 },
        ];
        ty
    }

    #[test]
    fn bits_sparse_encoding_matches_ruby_spec_exact_bytes() {
        let ty = alarm_state_type();
        assert_eq!(
            encode_bits(&ty, &Json::String("warning critical indeterminate".into()), false).unwrap(),
            Cbor::Array(vec![Cbor::Bytes(vec![0x04, 0x01]), Cbor::from(14), Cbor::Bytes(vec![0x01])])
        );
        assert_eq!(encode_bits(&ty, &Json::String("indeterminate".into()), false).unwrap(), Cbor::Array(vec![Cbor::from(16), Cbor::Bytes(vec![0x01])]));
        assert_eq!(encode_bits(&ty, &Json::String("".into()), false).unwrap(), Cbor::Array(vec![]));
    }

    #[test]
    fn bits_dense_encoding_is_a_plain_bytestring() {
        let ty = alarm_state_type();
        assert_eq!(encode_bits(&ty, &Json::String("critical under-repair".into()), false).unwrap(), Cbor::Bytes(vec![0x06]));
    }

    #[test]
    fn bits_decode_is_the_exact_inverse() {
        let ty = alarm_state_type();
        assert_eq!(decode_bits(&ty, &Cbor::Array(vec![Cbor::Bytes(vec![0x04, 0x01]), Cbor::from(14), Cbor::Bytes(vec![0x01])]), false).unwrap(), Json::String("critical warning indeterminate".into()));
        assert_eq!(decode_bits(&ty, &Cbor::Bytes(vec![0x06]), false).unwrap(), Json::String("under-repair critical".into()));
        assert_eq!(decode_bits(&ty, &Cbor::Array(vec![]), false).unwrap(), Json::String("".into()));
    }

    // Ported from yang-enc_spec.rb's decimal64 examples: fraction-digits
    // is always the exponent verbatim, and decode always round-trips
    // through a float (so a whole number still gets a trailing ".0").

    #[test]
    fn decimal64_exponent_is_always_fraction_digits() {
        let mut ty = TypeDef::new(Builtin::Decimal64);
        ty.fraction_digits = Some(2);
        assert_eq!(encode_decimal64(&ty, &Json::String("2.57".into()), false).unwrap(), Cbor::Tag(4, Box::new(Cbor::Array(vec![Cbor::from(-2), Cbor::from(257)]))));
        assert_eq!(encode_decimal64(&ty, &Json::String("25.7".into()), false).unwrap(), Cbor::Tag(4, Box::new(Cbor::Array(vec![Cbor::from(-2), Cbor::from(2570)]))));
        assert_eq!(encode_decimal64(&ty, &Json::String("257".into()), false).unwrap(), Cbor::Tag(4, Box::new(Cbor::Array(vec![Cbor::from(-2), Cbor::from(25700)]))));
    }

    #[test]
    fn decimal64_decode_always_shows_a_decimal_point() {
        let ty = TypeDef::new(Builtin::Decimal64);
        assert_eq!(decode_decimal64(&ty, &Cbor::Tag(4, Box::new(Cbor::Array(vec![Cbor::from(-2), Cbor::from(257)]))), false).unwrap(), Json::String("2.57".into()));
        assert_eq!(decode_decimal64(&ty, &Cbor::Tag(4, Box::new(Cbor::Array(vec![Cbor::from(-2), Cbor::from(25700)]))), false).unwrap(), Json::String("257.0".into()));
        assert_eq!(decode_decimal64(&ty, &Cbor::Tag(4, Box::new(Cbor::Array(vec![Cbor::from(-2), Cbor::from(0)]))), false).unwrap(), Json::String("0.0".into()));
    }

    // -- native validation / `--continue` leniency --------------------
    //
    // This is the coverage `json_validate`/`json_schemer` gave the Ruby
    // reference (see `type_to_cbor`'s own doc comment for why it's
    // native here instead): a hand-built catalog with a range, a length
    // + pattern, an `empty` leaf, and a keyed list, pinned independent
    // of whatever any real catalog happens to declare. Each check is
    // exercised both ways: a hard error without `--continue`, and a
    // warning-plus-best-effort encoding with it.

    fn validation_test_schema() -> Schema {
        let yang = r#"
            module test {
                namespace "urn:test";
                prefix t;

                container box {
                    leaf level {
                        type uint8 {
                            range "0..10";
                        }
                        mandatory true;
                    }
                    leaf mode {
                        type string {
                            length "1..4";
                            pattern "[a-z]+";
                        }
                    }
                    leaf armed {
                        type empty;
                    }
                    leaf-list tag {
                        type string;
                    }
                    leaf setting {
                        type union {
                            type boolean;
                            type uint8 {
                                range "0..10";
                            }
                        }
                    }
                    list item {
                        key "id";
                        leaf id {
                            type uint8;
                        }
                        leaf name {
                            type string;
                        }
                    }
                }
            }
        "#;
        let sid = r#"{
            "module-name": "test",
            "module-revision": "2026-01-01",
            "items": [
                {"namespace": "module", "identifier": "test", "sid": 1000},
                {"namespace": "data", "identifier": "/test:box", "sid": 1001},
                {"namespace": "data", "identifier": "/test:box/level", "sid": 1002},
                {"namespace": "data", "identifier": "/test:box/mode", "sid": 1003},
                {"namespace": "data", "identifier": "/test:box/armed", "sid": 1004},
                {"namespace": "data", "identifier": "/test:box/item", "sid": 1005},
                {"namespace": "data", "identifier": "/test:box/item/id", "sid": 1006},
                {"namespace": "data", "identifier": "/test:box/item/name", "sid": 1007},
                {"namespace": "data", "identifier": "/test:box/tag", "sid": 1008},
                {"namespace": "data", "identifier": "/test:box/setting", "sid": 1009}
            ]
        }"#;
        crate::schema::build(&[yang.to_string()], &[sid.to_string()]).expect("validation test schema builds")
    }

    fn box_node(schema: &Schema) -> NodeId {
        schema.find_child(schema.root, "test:box").expect("test:box exists")
    }

    /// A minimally-valid `box` body: just the one mandatory field.
    fn box_obj() -> serde_json::Map<String, Json> {
        let mut m = serde_json::Map::new();
        m.insert("level".to_string(), Json::from(5));
        m
    }

    #[test]
    fn range_violation_hard_fails_without_continue_and_degrades_with_it() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        let mut obj = box_obj();
        obj.insert("level".to_string(), Json::from(300));

        let e = encode_node_value(&schema, box_id, &Json::Object(obj.clone()), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("300"), "{}", e.0);
        assert!(e.0.contains("between 0 and 10"), "{}", e.0);

        // Continuing: the request still encodes (degraded, but present).
        assert!(encode_node_value(&schema, box_id, &Json::Object(obj), ContentFormat::Put, true).is_ok());
    }

    #[test]
    fn pattern_violation_hard_fails_without_continue_and_degrades_with_it() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        let mut obj = box_obj();
        obj.insert("mode".to_string(), Json::String("ABC".into()));

        let e = encode_node_value(&schema, box_id, &Json::Object(obj.clone()), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("ABC"), "{}", e.0);
        assert!(e.0.contains("[a-z]+"), "{}", e.0);

        assert!(encode_node_value(&schema, box_id, &Json::Object(obj), ContentFormat::Put, true).is_ok());
    }

    #[test]
    fn length_violation_is_rejected_with_the_actual_bound_shown() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        let mut obj = box_obj();
        obj.insert("mode".to_string(), Json::String("toolong".into()));

        let e = encode_node_value(&schema, box_id, &Json::Object(obj), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("7 character"), "{}", e.0);
        assert!(e.0.contains("between 1 and 4"), "{}", e.0);
    }

    #[test]
    fn unknown_field_hard_fails_without_continue_and_is_skipped_with_it() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        let mut obj = box_obj();
        obj.insert("nope".to_string(), Json::from(1));

        let e = encode_node_value(&schema, box_id, &Json::Object(obj.clone()), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("\"nope\""), "{}", e.0);
        assert!(e.0.contains("not a valid field"), "{}", e.0);

        let cbor = encode_node_value(&schema, box_id, &Json::Object(obj), ContentFormat::Put, true).unwrap();
        let decoded = decode_node_value(&schema, box_id, &cbor, ContentFormat::Put).unwrap();
        let decoded_obj = decoded.as_object().unwrap();
        assert!(!decoded_obj.contains_key("nope"), "unknown field should have been dropped, got {decoded:?}");
        assert!(decoded_obj.contains_key("level"));
    }

    #[test]
    fn missing_mandatory_field_hard_fails_without_continue_and_is_absent_with_it() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        let obj = serde_json::Map::new(); // no "level" at all

        let e = encode_node_value(&schema, box_id, &Json::Object(obj.clone()), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("\"level\""), "{}", e.0);
        assert!(e.0.contains("required"), "{}", e.0);

        let cbor = encode_node_value(&schema, box_id, &Json::Object(obj), ContentFormat::Put, true).unwrap();
        let decoded = decode_node_value(&schema, box_id, &cbor, ContentFormat::Put).unwrap();
        assert_eq!(decoded, Json::Object(serde_json::Map::new()), "the missing field should simply stay absent, not appear as null or a default");
    }

    #[test]
    fn missing_list_key_hard_fails_without_continue_and_is_absent_with_it() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        let mut item = serde_json::Map::new();
        item.insert("name".to_string(), Json::String("x".into()));
        let mut obj = box_obj();
        obj.insert("item".to_string(), Json::Array(vec![Json::Object(item)]));

        let e = encode_node_value(&schema, box_id, &Json::Object(obj.clone()), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("\"id\""), "{}", e.0);
        assert!(e.0.contains("key of this list"), "{}", e.0);

        assert!(encode_node_value(&schema, box_id, &Json::Object(obj), ContentFormat::Put, true).is_ok());
    }

    #[test]
    fn malformed_leaf_value_hard_fails_without_continue_and_passes_through_raw_with_it() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        let mut obj = box_obj();
        obj.insert("level".to_string(), Json::String("not-a-number".into()));

        let e = encode_node_value(&schema, box_id, &Json::Object(obj.clone()), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("expected a whole number"), "{}", e.0);

        let cbor = encode_node_value(&schema, box_id, &Json::Object(obj), ContentFormat::Put, true).unwrap();
        // Degraded: the raw string made it onto the wire verbatim (delta-
        // SID 1 == level's SID 1002 minus box's own SID 1001), not a
        // properly-typed integer.
        let map = cbor.as_map().expect("still a CBOR map");
        let (_, level_cbor) = map.iter().find(|(k, _)| cbor_as_i128(k) == Some(1)).expect("level's entry is present");
        assert_eq!(level_cbor, &Cbor::Text("not-a-number".to_string()));
    }

    #[test]
    fn empty_typed_leaf_requires_the_null_array_shape() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        let mut obj = box_obj();
        obj.insert("armed".to_string(), Json::Bool(true)); // wrong shape

        let e = encode_node_value(&schema, box_id, &Json::Object(obj.clone()), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("[null]"), "{}", e.0);

        obj.insert("armed".to_string(), Json::Array(vec![Json::Null])); // correct shape
        assert!(encode_node_value(&schema, box_id, &Json::Object(obj), ContentFormat::Put, false).is_ok());
    }

    // -- structural shape checks (container -> object, list -> array,
    // leaf-list -> array) -- what a JSON-Schema `type` keyword would
    // enforce, confirming the encoder's own type-shape checks give at
    // least the same coverage without one.

    #[test]
    fn container_shape_mismatch_hard_fails_without_continue_and_is_empty_with_it() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);

        let e = encode_node_value(&schema, box_id, &Json::from(5), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("needs an object"), "{}", e.0);

        // Continuing: `box` itself becomes an empty map -- matches
        // Ruby's own `result = {}` fallback, and the *same* shortcut it
        // takes: `encode_body`'s own missing-required-field check is
        // never reached here, since there's no body to check fields of.
        assert_eq!(encode_node_value(&schema, box_id, &Json::from(5), ContentFormat::Put, true).unwrap(), Cbor::Map(vec![]));
    }

    #[test]
    fn list_shape_mismatch_hard_fails_without_continue_and_is_empty_with_it() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        let item_id = schema.find_child(box_id, "item").expect("test:box/item exists");

        let e = encode_node_value(&schema, item_id, &Json::from(5), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("needs a list"), "{}", e.0);

        assert_eq!(encode_node_value(&schema, item_id, &Json::from(5), ContentFormat::Put, true).unwrap(), Cbor::Array(vec![]));
    }

    #[test]
    fn leaf_list_shape_mismatch_is_always_a_hard_error_regardless_of_continue() {
        // Unlike container/list, `leaf-list`'s own shape check has no
        // `--continue` leniency in Ruby either (an unguarded `json.map`
        // that would just crash on the wrong shape) -- replicated here
        // faithfully rather than "fixed", per this port's own stated
        // policy (see `encode_node_value`'s doc comment).
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        let tag_id = schema.find_child(box_id, "tag").expect("test:box/tag exists");

        assert!(encode_node_value(&schema, tag_id, &Json::from(5), ContentFormat::Put, false).is_err());
        assert!(encode_node_value(&schema, tag_id, &Json::from(5), ContentFormat::Put, true).is_err());
    }

    #[test]
    fn union_mismatch_names_the_value_and_the_allowed_types_not_an_internal_id() {
        let schema = validation_test_schema();
        let box_id = box_node(&schema);
        // `setting` is a `union { boolean; uint8 { range "0..10"; } }` --
        // a plain string matches neither member.
        let mut obj = box_obj();
        obj.insert("setting".to_string(), Json::String("nope".into()));

        let e = encode_node_value(&schema, box_id, &Json::Object(obj.clone()), ContentFormat::Put, false).unwrap_err();
        assert!(e.0.contains("\"nope\""), "{}", e.0);
        assert!(e.0.contains("boolean"), "{}", e.0);
        assert!(e.0.contains("uint8"), "{}", e.0);
        // The old message named an internal TypeId (a bare integer) --
        // never let one leak back in.
        assert!(!e.0.contains("type 0") && !e.0.contains("type 1"), "should not mention an internal type id: {}", e.0);

        // A value that *does* match a member still degrades sensibly
        // under --continue if that member's own constraints reject it
        // (300 doesn't fit uint8 0..10).
        obj.insert("setting".to_string(), Json::from(300));
        let e2 = encode_node_value(&schema, box_id, &Json::Object(obj.clone()), ContentFormat::Put, false).unwrap_err();
        assert!(e2.0.contains("between 0 and 10"), "{}", e2.0);
        assert!(encode_node_value(&schema, box_id, &Json::Object(obj), ContentFormat::Put, true).is_ok());
    }
}
