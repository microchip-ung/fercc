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
