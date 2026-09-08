// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

/// A decoded/to-be-encoded CBOR data item (RFC 8949 Section 3).
///
/// `Integer` holds the full range CBOR's major types 0/1 can represent
/// directly (`-2^64..=2^64-1`) in a plain `i128` -- there's no separate
/// bignum (tag 2/3) support, matching what this codebase ever actually
/// needs to round-trip.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Integer(i128),
    Bytes(Vec<u8>),
    Text(String),
    Bool(bool),
    Null,
    Tag(u64, Box<Value>),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
}

impl Value {
    pub fn as_map(&self) -> Option<&[(Value, Value)]> {
        match self {
            Value::Map(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

macro_rules! impl_from_int {
    ($($t:ty),*) => {
        $(
            impl From<$t> for Value {
                fn from(n: $t) -> Self {
                    Value::Integer(i128::from(n))
                }
            }
        )*
    };
}

impl_from_int!(i8, i16, i32, i64, i128, u8, u16, u32, u64);
