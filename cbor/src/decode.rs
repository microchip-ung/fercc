// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use crate::{Error, Value};

fn err(msg: impl Into<String>) -> Error {
    Error(msg.into())
}

/// Decode one CBOR data item starting at `bytes[*pos]`, advancing `*pos`
/// past it. Call in a loop (`while *pos < bytes.len()`) to decode a
/// sequence of items back to back.
///
/// Every byte access is checked -- malformed or truncated input always
/// produces an `Err`, never a panic, since this decodes bytes coming
/// straight off the wire from a device.
pub fn from_slice(bytes: &[u8], pos: &mut usize) -> Result<Value, Error> {
    let (major, info) = read_head_byte(bytes, pos)?;
    match major {
        0 => Ok(Value::Integer(i128::from(read_arg(bytes, pos, info)?))),
        1 => Ok(Value::Integer(-1 - i128::from(read_arg(bytes, pos, info)?))),
        2 => Ok(Value::Bytes(read_string_bytes(bytes, pos, info, 2)?)),
        3 => {
            let raw = read_string_bytes(bytes, pos, info, 3)?;
            String::from_utf8(raw).map(Value::Text).map_err(|_| err("this CBOR text string isn't valid UTF-8"))
        }
        4 => Ok(Value::Array(read_array(bytes, pos, info)?)),
        5 => Ok(Value::Map(read_map(bytes, pos, info)?)),
        6 => {
            let tag = read_arg(bytes, pos, info)?;
            let inner = from_slice(bytes, pos)?;
            Ok(Value::Tag(tag, Box::new(inner)))
        }
        7 => read_simple(info),
        _ => Err(err("internal error: a CBOR major type is always 0..=7")),
    }
}

/// The 3-bit major type and 5-bit additional-info field of one head byte
/// (Section 3.1).
fn read_head_byte(bytes: &[u8], pos: &mut usize) -> Result<(u8, u8), Error> {
    let b = *bytes.get(*pos).ok_or_else(|| err("unexpected end of CBOR data"))?;
    *pos += 1;
    Ok((b >> 5, b & 0x1f))
}

fn read_bytes<const N: usize>(bytes: &[u8], pos: &mut usize) -> Result<[u8; N], Error> {
    let end = pos.checked_add(N).ok_or_else(|| err("unexpected end of CBOR data"))?;
    let slice = bytes.get(*pos..end).ok_or_else(|| err("unexpected end of CBOR data"))?;
    let mut out = [0u8; N];
    out.copy_from_slice(slice);
    *pos = end;
    Ok(out)
}

/// Read the argument that follows a head byte, per its additional-info
/// field (Section 3.1): either the info value itself (0..=23), or an
/// explicit 1/2/4/8-byte big-endian integer.
fn read_arg(bytes: &[u8], pos: &mut usize, info: u8) -> Result<u64, Error> {
    match info {
        0..=23 => Ok(u64::from(info)),
        24 => Ok(u64::from(read_bytes::<1>(bytes, pos)?[0])),
        25 => Ok(u64::from(u16::from_be_bytes(read_bytes::<2>(bytes, pos)?))),
        26 => Ok(u64::from(u32::from_be_bytes(read_bytes::<4>(bytes, pos)?))),
        27 => Ok(u64::from_be_bytes(read_bytes::<8>(bytes, pos)?)),
        _ => Err(err(format!("this CBOR data uses an unsupported length encoding (additional info {info})"))),
    }
}

fn read_raw(bytes: &[u8], pos: &mut usize, len: u64) -> Result<Vec<u8>, Error> {
    let len = usize::try_from(len).map_err(|_| err("a length in this CBOR data is larger than this machine can address"))?;
    let end = pos.checked_add(len).ok_or_else(|| err("unexpected end of CBOR data"))?;
    let slice = bytes.get(*pos..end).ok_or_else(|| err("unexpected end of CBOR data"))?;
    *pos = end;
    Ok(slice.to_vec())
}

fn peek_is_break(bytes: &[u8], pos: &usize) -> Result<bool, Error> {
    let b = *bytes.get(*pos).ok_or_else(|| err("unexpected end of CBOR data"))?;
    Ok(b == 0xff)
}

/// Read the raw bytes of a byte string (major 2) or text string (major
/// 3), handling both its definite-length form and its indefinite-length
/// form (Section 3.2.3): a sequence of definite-length chunks of the
/// same major type, terminated by the break byte.
fn read_string_bytes(bytes: &[u8], pos: &mut usize, info: u8, major: u8) -> Result<Vec<u8>, Error> {
    if info != 31 {
        let len = read_arg(bytes, pos, info)?;
        return read_raw(bytes, pos, len);
    }
    let mut out = Vec::new();
    loop {
        if peek_is_break(bytes, pos)? {
            *pos += 1;
            return Ok(out);
        }
        let (chunk_major, chunk_info) = read_head_byte(bytes, pos)?;
        if chunk_major != major || chunk_info == 31 {
            return Err(err("an indefinite-length string contains a chunk that isn't a definite-length string of the same type"));
        }
        let len = read_arg(bytes, pos, chunk_info)?;
        out.extend(read_raw(bytes, pos, len)?);
    }
}

fn read_array(bytes: &[u8], pos: &mut usize, info: u8) -> Result<Vec<Value>, Error> {
    if info == 31 {
        let mut out = Vec::new();
        while !peek_is_break(bytes, pos)? {
            out.push(from_slice(bytes, pos)?);
        }
        *pos += 1;
        return Ok(out);
    }
    let count = read_arg(bytes, pos, info)?;
    let count = usize::try_from(count).map_err(|_| err("an array length in this CBOR data is larger than this machine can address"))?;
    // Reserve a bounded hint only -- a huge claimed count with few actual
    // bytes remaining still errors out item-by-item below, rather than
    // eagerly allocating an attacker-controlled amount of memory.
    let mut out = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        out.push(from_slice(bytes, pos)?);
    }
    Ok(out)
}

fn read_map(bytes: &[u8], pos: &mut usize, info: u8) -> Result<Vec<(Value, Value)>, Error> {
    if info == 31 {
        let mut out = Vec::new();
        while !peek_is_break(bytes, pos)? {
            let k = from_slice(bytes, pos)?;
            let v = from_slice(bytes, pos)?;
            out.push((k, v));
        }
        *pos += 1;
        return Ok(out);
    }
    let count = read_arg(bytes, pos, info)?;
    let count = usize::try_from(count).map_err(|_| err("a map length in this CBOR data is larger than this machine can address"))?;
    let mut out = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        let k = from_slice(bytes, pos)?;
        let v = from_slice(bytes, pos)?;
        out.push((k, v));
    }
    Ok(out)
}

/// Major type 7: booleans, null, undefined, and floats (Section 3.3).
/// `lmu_cbor` (the firmware's own CBOR library) has no float *encoder* at
/// all -- a real device can never put a float on the wire -- so there's
/// nothing to decode here: encountering one means the data isn't a
/// genuine device response, and that's worth an error rather than a
/// guess at a value.
fn read_simple(info: u8) -> Result<Value, Error> {
    match info {
        20 => Ok(Value::Bool(false)),
        21 => Ok(Value::Bool(true)),
        22 => Ok(Value::Null),
        23 => Ok(Value::Null), // "undefined" -- Value has no separate case for it
        25..=27 => Err(err("this CBOR data contains a floating-point value, which a VelocityDRIVE-SP device never sends -- this doesn't look like a genuine device response")),
        31 => Err(err("unexpected CBOR break byte outside an indefinite-length item")),
        _ => Err(err(format!("this CBOR data uses an unsupported simple value (additional info {info})"))),
    }
}
