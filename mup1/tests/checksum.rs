// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use mup1::checksum::*;
use mup1::ChecksumType;

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
