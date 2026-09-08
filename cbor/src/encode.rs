// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use crate::Value;

/// Encode `value` as CBOR (RFC 8949): always definite-length, always the
/// narrowest argument width that fits -- infallible, since encoding a
/// value we already hold in memory into bytes can't fail.
pub fn to_vec(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode_value(value, &mut out);
    out
}

fn encode_value(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Integer(i) if *i >= 0 => encode_head(0, *i as u64, out),
        Value::Integer(i) => encode_head(1, (-1 - *i) as u64, out),
        Value::Bytes(b) => {
            encode_head(2, b.len() as u64, out);
            out.extend_from_slice(b);
        }
        Value::Text(s) => {
            encode_head(3, s.len() as u64, out);
            out.extend_from_slice(s.as_bytes());
        }
        Value::Array(items) => {
            encode_head(4, items.len() as u64, out);
            for item in items {
                encode_value(item, out);
            }
        }
        Value::Map(entries) => {
            encode_head(5, entries.len() as u64, out);
            for (k, v) in entries {
                encode_value(k, out);
                encode_value(v, out);
            }
        }
        Value::Tag(tag, inner) => {
            encode_head(6, *tag, out);
            encode_value(inner, out);
        }
        Value::Bool(false) => out.push(0xf4),
        Value::Bool(true) => out.push(0xf5),
        Value::Null => out.push(0xf6),
    }
}

/// Write a major-type/argument head (Section 3.1): the 3-bit major type,
/// then the argument in the narrowest of the 5 encodings that fits.
fn encode_head(major: u8, arg: u64, out: &mut Vec<u8>) {
    let top = major << 5;
    if arg < 24 {
        out.push(top | arg as u8);
    } else if arg <= u64::from(u8::MAX) {
        out.push(top | 24);
        out.push(arg as u8);
    } else if arg <= u64::from(u16::MAX) {
        out.push(top | 25);
        out.extend_from_slice(&(arg as u16).to_be_bytes());
    } else if arg <= u64::from(u32::MAX) {
        out.push(top | 26);
        out.extend_from_slice(&(arg as u32).to_be_bytes());
    } else {
        out.push(top | 27);
        out.extend_from_slice(&arg.to_be_bytes());
    }
}
