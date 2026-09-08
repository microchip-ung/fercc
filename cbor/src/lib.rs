// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! A from-scratch CBOR (RFC 8949) codec: no `unsafe`, no dependencies.
//!
//! Written to replace `ciborium` (which pulls in `half` for no benefit
//! this codebase actually uses), matching its *capability* rather than
//! its API: definite-length encoding of every major type, plus decoding
//! of both definite- and indefinite-length arrays/maps/strings -- real
//! VelocityDRIVE-SP firmware (`lma_cc_cbor.c`, via its `lmu_cbor`
//! library) emits indefinite-length containers, so decoding them is a
//! real requirement even though nothing here ever needs to *encode* one.
//! There's no `Value::Float` at all, and no float support in either
//! direction: the firmware's own CBOR encoder has no float support, so
//! a real device can never send one, and nothing in this codebase ever
//! needs to send one either. Encountering a float on the wire is
//! treated as a sign the data isn't a genuine device response.

mod decode;
mod encode;
mod value;

pub use decode::from_slice;
pub use encode::to_vec;
pub use value::Value;

#[derive(Debug)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        assert_eq!(s.len() % 2, 0);
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    /// Decode the whole buffer as exactly one item, asserting nothing is
    /// left over.
    fn decode_one(bytes: &[u8]) -> Value {
        let mut pos = 0;
        let v = from_slice(bytes, &mut pos).unwrap();
        assert_eq!(pos, bytes.len(), "leftover bytes after decoding");
        v
    }

    /// Assert `bytes` (as hex) decodes to `value`, and that re-encoding
    /// `value` reproduces the same bytes -- RFC 8949 Appendix A's
    /// examples are all already minimal-width, so this checks both
    /// directions against one authoritative vector.
    fn roundtrip(hex_bytes: &str, value: Value) {
        let bytes = hex(hex_bytes);
        assert_eq!(decode_one(&bytes), value, "decoding {hex_bytes}");
        assert_eq!(to_vec(&value), bytes, "encoding {value:?}");
    }

    // --- RFC 8949 Appendix A: unsigned/negative integers, various widths ---

    #[test]
    fn integers() {
        roundtrip("00", Value::from(0u8));
        roundtrip("01", Value::from(1u8));
        roundtrip("0a", Value::from(10u8));
        roundtrip("17", Value::from(23u8));
        roundtrip("1818", Value::from(24u8));
        roundtrip("1819", Value::from(25u8));
        roundtrip("1864", Value::from(100u8));
        roundtrip("1903e8", Value::from(1000u16));
        roundtrip("1a000f4240", Value::from(1_000_000u32));
        roundtrip("1b000000e8d4a51000", Value::from(1_000_000_000_000i64));
        roundtrip("1bffffffffffffffff", Value::Integer(i128::from(u64::MAX)));
        roundtrip("20", Value::from(-1i8));
        roundtrip("29", Value::from(-10i8));
        roundtrip("3863", Value::from(-100i8));
        roundtrip("3903e7", Value::from(-1000i16));
        roundtrip("3b7fffffffffffffff", Value::from(i64::MIN));
    }

    #[test]
    fn integer_width_boundaries() {
        roundtrip("18ff", Value::from(255u8));
        roundtrip("190100", Value::from(256u16));
        roundtrip("19ffff", Value::from(65535u32));
        roundtrip("1a00010000", Value::from(65536u32));
        roundtrip("1affffffff", Value::from(u32::MAX));
        roundtrip("1b0000000100000000", Value::from(u64::from(u32::MAX) + 1));
    }

    // --- text/byte strings ---

    #[test]
    fn strings() {
        roundtrip("60", Value::Text(String::new()));
        roundtrip("6161", Value::Text("a".to_string()));
        roundtrip("6449455446", Value::Text("IETF".to_string()));
        roundtrip("62225c", Value::Text("\"\\".to_string()));
        roundtrip("40", Value::Bytes(vec![]));
        roundtrip("4401020304", Value::Bytes(vec![1, 2, 3, 4]));
    }

    #[test]
    fn text_string_must_be_valid_utf8() {
        let mut pos = 0;
        let err = from_slice(&hex("61ff"), &mut pos).unwrap_err();
        assert!(err.to_string().contains("UTF-8"));
    }

    // --- arrays/maps, including nesting ---

    #[test]
    fn arrays_and_maps() {
        roundtrip("80", Value::Array(vec![]));
        roundtrip("83010203", Value::Array(vec![Value::from(1u8), Value::from(2u8), Value::from(3u8)]));
        roundtrip(
            "8301820203820405",
            Value::Array(vec![
                Value::from(1u8),
                Value::Array(vec![Value::from(2u8), Value::from(3u8)]),
                Value::Array(vec![Value::from(4u8), Value::from(5u8)]),
            ]),
        );
        roundtrip("a0", Value::Map(vec![]));
        roundtrip("a201020304", Value::Map(vec![(Value::from(1u8), Value::from(2u8)), (Value::from(3u8), Value::from(4u8))]));
        roundtrip(
            "a26161016162820203",
            Value::Map(vec![
                (Value::Text("a".to_string()), Value::from(1u8)),
                (Value::Text("b".to_string()), Value::Array(vec![Value::from(2u8), Value::from(3u8)])),
            ]),
        );
    }

    // --- tags ---

    #[test]
    fn tags() {
        roundtrip("c11a514b67b0", Value::Tag(1, Box::new(Value::from(1_363_896_240u32))));
        assert_eq!(decode_one(&hex("c074323031332d30332d32315432303a30343a30305a")), Value::Tag(0, Box::new(Value::Text("2013-03-21T20:04:00Z".to_string()))));
    }

    // --- booleans, null, undefined ---

    #[test]
    fn simple_values() {
        roundtrip("f4", Value::Bool(false));
        roundtrip("f5", Value::Bool(true));
        roundtrip("f6", Value::Null);
        assert_eq!(decode_one(&hex("f7")), Value::Null); // undefined, folded into Null
    }

    // --- floats: unsupported in both directions (see module doc comment) ---

    #[test]
    fn decoding_any_float_width_is_an_error() {
        // A real device can never send a float (lmu_cbor's encoder has
        // no float support at all), so seeing one on the wire means the
        // data isn't a genuine device response -- an error, not a
        // best-effort guess at a value.
        for bad in ["f93c00", "fa47c35000", "fb3ff199999999999a"] {
            let mut pos = 0;
            let err = from_slice(&hex(bad), &mut pos).unwrap_err();
            assert!(err.to_string().contains("never sends"), "{bad}: {err}");
        }
    }

    // --- indefinite-length containers: the real firmware-capability
    // requirement (`lma_cc_cbor.c` uses `LMU_CBOR_INDEFINITE_LENGTH`) ---

    #[test]
    fn indefinite_length_byte_string() {
        assert_eq!(decode_one(&hex("5f42010243030405ff")), Value::Bytes(vec![1, 2, 3, 4, 5]));
    }

    #[test]
    fn indefinite_length_text_string() {
        assert_eq!(decode_one(&hex("7f657374726561646d696e67ff")), Value::Text("streaming".to_string()));
    }

    #[test]
    fn indefinite_length_array() {
        assert_eq!(
            decode_one(&hex("9f010203ff")),
            Value::Array(vec![Value::from(1u8), Value::from(2u8), Value::from(3u8)])
        );
        assert_eq!(decode_one(&hex("9fff")), Value::Array(vec![]));
        // Mixed definite/indefinite nesting.
        assert_eq!(
            decode_one(&hex("83018202039f0405ff")),
            Value::Array(vec![
                Value::from(1u8),
                Value::Array(vec![Value::from(2u8), Value::from(3u8)]),
                Value::Array(vec![Value::from(4u8), Value::from(5u8)]),
            ])
        );
    }

    #[test]
    fn indefinite_length_map() {
        assert_eq!(
            decode_one(&hex("bf61610161629f0203ffff")),
            Value::Map(vec![
                (Value::Text("a".to_string()), Value::from(1u8)),
                (Value::Text("b".to_string()), Value::Array(vec![Value::from(2u8), Value::from(3u8)])),
            ])
        );
    }

    #[test]
    fn indefinite_length_string_chunk_must_match_major_type() {
        // A text-string chunk (0x61 "a") inside an indefinite byte
        // string (0x5f) isn't allowed.
        let mut pos = 0;
        let err = from_slice(&hex("5f6161ff"), &mut pos).unwrap_err();
        assert!(err.to_string().contains("indefinite-length string"));
    }

    // --- malformed/truncated input: errors, never panics ---

    #[test]
    fn empty_input_is_an_error() {
        let mut pos = 0;
        assert!(from_slice(&[], &mut pos).is_err());
    }

    #[test]
    fn truncated_length_prefix_is_an_error() {
        let mut pos = 0;
        assert!(from_slice(&hex("19ff"), &mut pos).is_err()); // claims a u16 length, only 1 byte follows
    }

    #[test]
    fn byte_string_length_past_end_of_input_is_an_error() {
        let mut pos = 0;
        assert!(from_slice(&hex("4401"), &mut pos).is_err()); // claims 4 bytes, only 1 follows
    }

    #[test]
    fn unterminated_indefinite_array_is_an_error() {
        let mut pos = 0;
        assert!(from_slice(&hex("9f0102"), &mut pos).is_err()); // never hits a break byte
    }

    #[test]
    fn reserved_additional_info_is_an_error() {
        let mut pos = 0;
        assert!(from_slice(&[0x1c], &mut pos).is_err()); // major 0, additional info 28 (reserved)
    }

    #[test]
    fn unsupported_one_byte_simple_value_is_an_error() {
        let mut pos = 0;
        assert!(from_slice(&hex("f818"), &mut pos).is_err()); // major 7, additional info 24
    }

    #[test]
    fn bare_break_byte_is_an_error() {
        let mut pos = 0;
        assert!(from_slice(&[0xff], &mut pos).is_err());
    }

    // --- sequences: repeated top-level decode, as codec.rs's
    // cbor_seq_from_bytes does ---

    #[test]
    fn decoding_a_sequence() {
        let bytes = hex("0102"); // two 1-byte integers back to back
        let mut pos = 0;
        let mut items = Vec::new();
        while pos < bytes.len() {
            items.push(from_slice(&bytes, &mut pos).unwrap());
        }
        assert_eq!(items, vec![Value::from(1u8), Value::from(2u8)]);
    }

    // --- accessors ---

    #[test]
    fn accessors() {
        assert_eq!(Value::Text("x".to_string()).as_text(), Some("x"));
        assert_eq!(Value::Integer(5).as_text(), None);
        assert_eq!(Value::Bytes(vec![1, 2]).as_bytes(), Some(&[1u8, 2u8][..]));
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
        assert_eq!(Value::Array(vec![Value::from(1u8)]).as_array(), Some(&[Value::from(1u8)][..]));
        assert_eq!(
            Value::Map(vec![(Value::from(1u8), Value::from(2u8))]).as_map(),
            Some(&[(Value::from(1u8), Value::from(2u8))][..])
        );
    }
}
