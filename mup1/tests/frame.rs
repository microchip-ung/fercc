// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use mup1::frame::*;
use mup1::ChecksumType;

const CR: u8 = 0x0D;
const LF: u8 = 0x0A;
const RAW_BUF_CAP: usize = 1 + 1 + 2 * MAX_DATA_SIZE + 2 + 5;

fn roundtrip(checksum_type: ChecksumType, type_byte: u8, data: &[u8]) -> Frame {
    let wire = encode(checksum_type, type_byte, data);
    let mut dec = Decoder::new(checksum_type);
    let mut last = InjectStatus::default();
    for &b in &wire {
        last = dec.inject(b);
    }
    assert!(
        last.frame_ready,
        "expected a ready frame after full wire bytes"
    );
    let f = dec.get().expect("a frame");
    assert!(dec.get().is_none(), "exactly one frame expected");
    f
}

#[test]
fn plain_data_no_escaping_odd_size_single_eof() {
    let wire = encode(ChecksumType::Internet, b'X', b"hello");
    assert_eq!(wire[0], SOF);
    assert_eq!(wire[wire.len() - 2], CR);
    assert_eq!(wire[wire.len() - 1], LF);
    let f = roundtrip(ChecksumType::Internet, b'X', b"hello");
    assert_eq!(f.type_byte, b'X');
    assert_eq!(f.data, b"hello");
}

#[test]
fn even_size_data_with_internet_checksum_doubles_eof() {
    let wire = encode(ChecksumType::Internet, b'Y', b"abcd");
    // bytes: SOF, TYPE, 4 data bytes (no escaping needed) -> EOF EOF
    let mut i = 2 + 4;
    let mut eof_count = 0;
    while wire[i] == EOF {
        eof_count += 1;
        i += 1;
    }
    assert_eq!(eof_count, 2);
    let f = roundtrip(ChecksumType::Internet, b'Y', b"abcd");
    assert_eq!(f.data, b"abcd");
}

#[test]
fn escapes_every_special_byte_and_round_trips() {
    let data = [SOF, EOF, ESC, 0x00, 0xFF, 0x41];
    let wire = encode(ChecksumType::Internet, b'Z', &data);
    assert!(
        wire.len() > 2 + data.len(),
        "some escaping must have occurred"
    );
    let f = roundtrip(ChecksumType::Internet, b'Z', &data);
    assert_eq!(f.data, data);
}

#[test]
fn crc32_checksum_round_trips() {
    let f = roundtrip(ChecksumType::Crc32, b'C', b"abc");
    assert_eq!(f.data, b"abc");
}

#[test]
fn rejects_a_genuinely_wrong_checksum() {
    let mut dec = Decoder::new(ChecksumType::Internet);
    for b in [SOF, b'A', b'A', EOF, b'1', b'2', b'3', b'4'] {
        dec.inject(b);
    }
    let f = dec.get().expect("recovered as raw");
    assert_eq!(f.type_byte, TYPE_RAW);
}

#[test]
fn checksum_zero_sentinel_is_accepted_regardless_of_content() {
    // SOF 'A' "x" EOF EOF "0000" (odd data size -> actually 1 byte is
    // odd, so single EOF; use size 2 to also cover EOF-doubling).
    let mut dec = Decoder::new(ChecksumType::Internet);
    for b in [SOF, b'A', b'x', b'y', EOF, EOF, b'0', b'0', b'0', b'0'] {
        dec.inject(b);
    }
    let f = dec.get().expect("frame");
    assert_eq!(f.type_byte, b'A');
    assert_eq!(f.data, b"xy");
}

#[test]
fn illegal_escape_target_does_not_abort_and_is_preserved_verbatim_on_failure() {
    let mut dec = Decoder::new(ChecksumType::Internet);
    // SOF 'X' ESC 'A' 'B' -- 'A' is not a legal escape target, but the
    // frame keeps going; two logical data bytes (even) -> doubled EOF.
    for b in [SOF, b'X', ESC, b'A', b'B'] {
        dec.inject(b);
    }
    for b in [EOF, EOF, b'0', b'0', b'0', b'1'] {
        dec.inject(b);
    }
    let f = dec.get().expect("recovered as raw, not lost");
    assert_eq!(f.type_byte, TYPE_RAW);
    // SOF, TYPE, ESC, 'A', 'B', EOF, EOF, 4 checksum digits.
    assert_eq!(f.data.len(), 11);
    assert_eq!(f.data[2], ESC);
    assert_eq!(f.data[3], b'A');
}

#[test]
fn idle_byte_with_no_preceding_sof_becomes_raw() {
    let mut dec = Decoder::new(ChecksumType::Internet);
    let st = dec.inject(b'A');
    assert!(st.frame_ready);
    let f = dec.get().unwrap();
    assert_eq!(f.type_byte, TYPE_RAW);
    assert_eq!(f.data, vec![b'A']);
}

#[test]
fn overflow_of_raw_buffer_drops_exactly_one_byte() {
    let mut dec = Decoder::new(ChecksumType::Internet);
    dec.inject(SOF);
    dec.inject(b'A');
    for _ in 0..(RAW_BUF_CAP - 2) {
        let st = dec.inject(b'B');
        assert!(!st.buffer_full);
    }
    let st = dec.inject(b'B');
    assert!(st.buffer_full);
    assert!(st.frame_ready);
    assert_eq!(dec.drop_count(), 1);
}

#[test]
fn tick_times_out_a_stuck_attempt_and_progress_resets_it() {
    let mut dec = Decoder::new(ChecksumType::Internet);
    dec.set_timeout_ms(100);
    dec.inject(SOF);
    dec.inject(b'X');
    assert!(!dec.tick(0).frame_ready);
    assert!(!dec.tick(99).frame_ready);

    dec.inject(b'A'); // progress at t=99 (implicitly "now")
    assert!(!dec.tick(150).frame_ready, "reset by the progress above");
    assert!(dec.tick(250).frame_ready);
}

#[test]
fn trailer_cr_lf_after_success_is_consumed_not_raw() {
    let mut dec = Decoder::new(ChecksumType::Internet);
    for b in [SOF, b'A', b'x', EOF, b'0', b'0', b'0', b'0', CR, LF] {
        dec.inject(b);
    }
    let f = dec.get().unwrap();
    assert_eq!(f.type_byte, b'A');
    assert!(
        dec.get().is_none(),
        "trailer must not surface as raw content"
    );
}

#[test]
fn multiple_frames_come_out_in_order() {
    let mut dec = Decoder::new(ChecksumType::Internet);
    for b in [SOF, b'1', b'a', b'a', b'a', EOF, b'0', b'0', b'0', b'0'] {
        dec.inject(b);
    }
    for b in [SOF, b'2', b'b', b'b', EOF, EOF, b'0', b'0', b'0', b'0'] {
        dec.inject(b);
    }
    let f1 = dec.get().unwrap();
    assert_eq!((f1.type_byte, &f1.data[..]), (b'1', &b"aaa"[..]));
    let f2 = dec.get().unwrap();
    assert_eq!((f2.type_byte, &f2.data[..]), (b'2', &b"bb"[..]));
    assert!(dec.get().is_none());
}

#[test]
fn get_on_empty_decoder_is_none() {
    let mut dec = Decoder::new(ChecksumType::Internet);
    assert!(dec.get().is_none());
}
