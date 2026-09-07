// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! MUP1 frame checksums.
//!
//! Ported from `client-lib/src/lm_mup1.c` + `utils/src/lm_utils_crc32.c`
//! (cross-checked against `support/libeasy/handler/mup1.rb` and
//! `docs/sw_reqs/mup1.adoc`), which is the authoritative, hardware-verified
//! reference for both checksum types used on the wire.

/// Which of the two MUP1 checksum algorithms a connection uses. Fixed per
/// connection (set via `--checksum-type` on `mup1cc`), never
/// autodetected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumType {
    /// RFC 1071-style 16-bit one's-complement "internet checksum", encoded
    /// as 4 lowercase ASCII hex digits. This is the MUP1 default.
    Internet,
    /// Standard CRC-32/ISO-HDLC (poly 0xEDB88320, init/xorout 0xFFFFFFFF),
    /// encoded as 5 bytes via a bespoke base-128 digit-to-byte table.
    Crc32,
}

/// Number of wire bytes a checksum occupies for this type.
pub fn encoded_len(t: ChecksumType) -> usize {
    match t {
        ChecksumType::Internet => 4,
        ChecksumType::Crc32 => 5,
    }
}

/// Running checksum accumulator, fed one *logical* (post-unescape on RX,
/// pre-escape on TX) byte at a time in frame order: SOF, TYPE, each DATA
/// byte, EOF, and (Internet checksum only) a second EOF when the data
/// length is even.
#[derive(Debug, Clone, Copy)]
pub enum Running {
    Internet { acc: u32, pos: u16 },
    Crc32 { crc: u32 },
}

impl Running {
    pub fn new(t: ChecksumType) -> Self {
        match t {
            ChecksumType::Internet => Running::Internet { acc: 0, pos: 0 },
            ChecksumType::Crc32 => Running::Crc32 { crc: 0xFFFF_FFFF },
        }
    }

    /// Feed one logical byte. `pos` for the Internet checksum determines
    /// whether the byte lands in the high or low half of a 16-bit word:
    /// odd position -> low byte, even position -> high byte (lm_mup1.c
    /// `checksum_update`).
    pub fn update(&mut self, byte: u8) {
        match self {
            Running::Internet { acc, pos } => {
                if *pos % 2 != 0 {
                    *acc += byte as u32;
                } else {
                    *acc += (byte as u32) << 8;
                }
                *pos += 1;
            }
            Running::Crc32 { crc } => {
                let idx = ((*crc ^ byte as u32) & 0xFF) as usize;
                *crc = (*crc >> 8) ^ CRC32_TABLE[idx];
            }
        }
    }

    /// Render the final checksum as its on-wire ASCII bytes (4 hex digits
    /// for Internet, 5 base-128 bytes for CRC32).
    pub fn finalize(self) -> Vec<u8> {
        match self {
            Running::Internet { acc, .. } => {
                let sum = inet_fold(acc);
                let mut out = Vec::with_capacity(4);
                for i in 0..4u32 {
                    let nibble = ((sum >> (12 - 4 * i)) & 0xF) as u8;
                    out.push(if nibble < 10 {
                        b'0' + nibble
                    } else {
                        b'a' + nibble - 10
                    });
                }
                out
            }
            Running::Crc32 { crc } => {
                let final_crc = crc ^ 0xFFFF_FFFF;
                base128_encode(final_crc).to_vec()
            }
        }
    }
}

/// Fold carries twice (per RFC 1071 / `lmu_inet_chksum`) and one's-complement,
/// returning the 16-bit checksum.
fn inet_fold(acc: u32) -> u16 {
    let mut sum = acc;
    sum = (sum >> 16) + (sum & 0xFFFF);
    sum = (sum >> 16) + (sum & 0xFFFF);
    !(sum as u16)
}

/// The "0000" checksum value is a magic sentinel meaning "ignore the
/// checksum" (docs/sw_reqs/mup1.adoc, lm_mup1.c `parser_checksum_ok`) --
/// checked only for the Internet checksum type, which is the only one with
/// an all-zero-nibble encoding that's otherwise legal ASCII.
pub fn is_ignore_sentinel(t: ChecksumType, wire_bytes: &[u8]) -> bool {
    t == ChecksumType::Internet && wire_bytes.iter().all(|&b| b == b'0')
}

// -- CRC32 (poly 0xEDB88320, reflected) -------------------------------------

const CRC32_TABLE: [u32; 256] = build_crc32_table();

const fn build_crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut j = 0;
        while j < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            j += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}

// -- base-128 digit <-> byte mapping (utils/src/lm_utils_crc32.c) ----------
//
// A 32-bit CRC is split into five 7-bit digits (big-endian, MSB-first) and
// each digit is mapped through this 128-entry table into a byte outside
// the ranges used by MUP1 control bytes (SOF/EOF/ESC/0x00/0xFF). Ported
// verbatim from `base128_encoding_table` -- this exact table, not a
// reinvented one, is what real firmware expects.
const BASE128_ENCODE: [u8; 128] = [
    0x2F, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3F, 0x40, 0x41, 0x42, 0x43,
    0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0x53,
    0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
    0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
    0x7A, 0xBF, 0xC0, 0xC1, 0xC2, 0xC3, 0xC4, 0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xCB, 0xCC, 0xCD,
    0xCE, 0xCF, 0xD0, 0xD1, 0xD2, 0xD3, 0xD4, 0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xDB, 0xDC, 0xDD,
    0xDE, 0xDF, 0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xEB, 0xEC, 0xED,
    0xEE, 0xEF, 0xF0, 0xF1, 0xF2, 0xF3, 0xF4, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xFD,
];

fn base128_encode(x: u32) -> [u8; 5] {
    let mut digits = [0u8; 5];
    let mut v = x;
    for i in (0..5).rev() {
        digits[i] = (v & 0x7F) as u8;
        v >>= 7;
    }
    let mut out = [0u8; 5];
    for i in 0..5 {
        out[i] = BASE128_ENCODE[digits[i] as usize];
    }
    out
}

/// Reverse lookup, derived from `BASE128_ENCODE` rather than hand-transcribed
/// (the C source's decode table is a 256-entry sparse array; deriving it
/// avoids a second chance to mistranscribe one of the 128 mappings).
fn base128_decode_digit(byte: u8) -> Option<u8> {
    BASE128_ENCODE.iter().position(|&b| b == byte).map(|i| i as u8)
}

/// Decode a 5-byte base-128 checksum back into the 32-bit CRC it encodes.
pub fn base128_decode(encoded: &[u8]) -> Option<u32> {
    if encoded.len() != 5 {
        return None;
    }
    let mut x: u32 = 0;
    for (i, &b) in encoded.iter().enumerate() {
        let digit = base128_decode_digit(b)? as u32;
        x |= digit;
        if i != 4 {
            x <<= 7;
        }
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base128_round_trips_every_crc_value_shape() {
        for &v in &[0u32, 1, 0x7F, 0x80, 0xFFFF_FFFF, 0x1234_5678, 0xEDB8_8320] {
            let enc = base128_encode(v);
            assert_eq!(base128_decode(&enc), Some(v));
        }
    }

    #[test]
    fn internet_checksum_of_empty_is_all_ones_complement() {
        // sum=0 -> fold -> 0 -> one's complement -> 0xFFFF
        let r = Running::new(ChecksumType::Internet);
        assert_eq!(r.finalize(), b"ffff");
    }

    #[test]
    fn ignore_sentinel_only_applies_to_internet() {
        assert!(is_ignore_sentinel(ChecksumType::Internet, b"0000"));
        assert!(!is_ignore_sentinel(ChecksumType::Internet, b"0001"));
        assert!(!is_ignore_sentinel(ChecksumType::Crc32, b"00000"));
    }
}
