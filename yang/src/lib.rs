// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! From-scratch YANG (RFC 7950) parsing and schema interpretation, plus
//! the SID-CBOR wire codec (RFC 9254 / RFC 9595) used by CORECONF.

pub mod catalog;
pub mod codec;
pub mod json_schema;
pub mod parser;
pub mod schema;
pub mod sid;
