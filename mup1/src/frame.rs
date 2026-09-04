//! MUP1 frame encode/decode.
//!
//! Ported from the state machine in `client-lib/src/lm_mup1.c`
//! (`lm_mup1_put`/`lm_mup1_inject`/`lm_mup1_tick`/`lm_mup1_get`), which is
//! the hardware-verified reference (cross-checked against
//! `support/libeasy/handler/mup1.rb` and `docs/sw_reqs/mup1.adoc`). The
//! embedded original uses fixed-size buffers for a no-alloc MCU; this port
//! uses growable `Vec`s but keeps the exact same state transitions, byte
//! offsets, and recovery behavior so the ported unit tests (from
//! `client-lib/test/ut/lm_ut_mup1.c`) hold unchanged.

use std::collections::VecDeque;

use crate::checksum::{self, ChecksumType, Running};

pub const SOF: u8 = 0x3E;
pub const EOF: u8 = 0x3C;
pub const ESC: u8 = 0x5C;
const ESCAPED_00: u8 = 0x30;
const ESCAPED_FF: u8 = 0x46;
const CR: u8 = 0x0D;
const LF: u8 = 0x0A;

/// Maximum MUP1 payload size (`LM_MUP1_DATA_SIZE`).
pub const MAX_DATA_SIZE: usize = 300;

/// Sentinel `Frame::type_byte` for recovered raw/non-frame content
/// (`LM_MUP1_TYPE_RAW`) -- never a real protocol type byte on the wire.
pub const TYPE_RAW: u8 = 0xFF;

/// Cap on the combined settled+in-progress verbatim byte buffer, matching
/// the embedded reference's `LM_MUP1_RAW_BUF_SIZE` (worst case: SOF + TYPE +
/// fully-escaped DATA + 2 EOFs + 5 checksum bytes). Bounds memory growth on
/// pathological input; ordinary traffic never approaches it.
const RAW_BUF_CAP: usize = 1 + 1 + 2 * MAX_DATA_SIZE + 2 + 5;

fn needs_escape(b: u8) -> bool {
    matches!(b, SOF | EOF | ESC | 0x00 | 0xFF)
}

fn escape_byte(b: u8) -> u8 {
    match b {
        0x00 => ESCAPED_00,
        0xFF => ESCAPED_FF,
        other => other,
    }
}

fn unescape_byte(b: u8) -> u8 {
    match b {
        ESCAPED_00 => 0x00,
        ESCAPED_FF => 0xFF,
        other => other,
    }
}

/// One MUP1 frame: a protocol type byte plus its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub type_byte: u8,
    pub data: Vec<u8>,
}

/// Encode `data` as a complete MUP1 frame ready to write to the wire,
/// including the trailing CR LF (display-only, no protocol meaning --
/// `lm_mup1_put` always emits it, and the decoder always swallows it after
/// a successful decode; matching this exactly is simplest and harmless).
pub fn encode(checksum_type: ChecksumType, type_byte: u8, data: &[u8]) -> Vec<u8> {
    let mut running = Running::new(checksum_type);
    let mut out = Vec::with_capacity(data.len() * 2 + 16);

    running.update(SOF);
    out.push(SOF);

    running.update(type_byte);
    out.push(type_byte);

    for &b in data {
        running.update(b);
        if needs_escape(b) {
            out.push(ESC);
            out.push(escape_byte(b));
        } else {
            out.push(b);
        }
    }

    running.update(EOF);
    out.push(EOF);
    if checksum_type == ChecksumType::Internet && data.len().is_multiple_of(2) {
        running.update(EOF);
        out.push(EOF);
    }

    out.extend_from_slice(&running.finalize());
    out.push(CR);
    out.push(LF);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Sof,
    Data,
    DataEsc,
    Eof,
    Checksum,
    PostFrame,
    PostFrameCr,
}

/// Status flags returned by [`Decoder::inject`] / [`Decoder::tick`],
/// mirroring `LM_MUP1_STATUS_*`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InjectStatus {
    /// A frame (decoded or recovered raw) is ready via [`Decoder::get`].
    pub frame_ready: bool,
    /// The frame that was just completed failed its checksum (and was
    /// recovered as raw content, not lost).
    pub checksum_error: bool,
    /// The just-injected byte could not fit; one byte was dropped.
    pub buffer_full: bool,
}

/// Byte-fed MUP1 frame decoder. Feed wire bytes one at a time via
/// [`inject`](Decoder::inject) (from a UART RX interrupt, in the embedded
/// original -- here, from whatever reads the serial port), call
/// [`tick`](Decoder::tick) periodically with a monotonic millisecond clock
/// to recover stalled partial frames, and drain completed frames (or
/// recovered raw content, `type_byte == TYPE_RAW`) via
/// [`get`](Decoder::get).
///
/// No byte is ever silently discarded except on the `RAW_BUF_CAP` overflow
/// case (`drop_count`); anything that isn't a valid frame comes back as
/// `TYPE_RAW` content instead.
pub struct Decoder {
    checksum_type: ChecksumType,
    state: State,

    /// Verbatim wire bytes. `raw[..raw_head]` is settled (returnable via
    /// `get()`); `raw[raw_head..]` is the current open attempt.
    raw: Vec<u8>,
    raw_head: usize,

    /// `raw` index where the open attempt's DATA section ends.
    data_end: usize,
    /// Count of logical (post-unescape) data bytes fed for the open
    /// attempt, i.e. what would become `Frame::data.len()`.
    data_len: usize,

    running: Running,
    checksum_len: usize,
    checksum_bytes: Vec<u8>,

    frames: VecDeque<Frame>,
    drop_count: u64,

    timeout_ms: u64,
    open_len_at_last_tick: usize,
    time_of_last_progress: u64,
}

impl Decoder {
    pub fn new(checksum_type: ChecksumType) -> Self {
        Self {
            checksum_type,
            state: State::Idle,
            raw: Vec::new(),
            raw_head: 0,
            data_end: 0,
            data_len: 0,
            running: Running::new(checksum_type),
            checksum_len: 0,
            checksum_bytes: Vec::new(),
            frames: VecDeque::new(),
            drop_count: 0,
            timeout_ms: 500,
            open_len_at_last_tick: 0,
            time_of_last_progress: 0,
        }
    }

    pub fn set_timeout_ms(&mut self, ms: u64) {
        self.timeout_ms = ms;
    }

    pub fn drop_count(&self) -> u64 {
        self.drop_count
    }

    fn status(&self, checksum_error: bool, buffer_full: bool) -> InjectStatus {
        InjectStatus {
            frame_ready: !self.frames.is_empty() || self.raw_head > 0,
            checksum_error,
            buffer_full,
        }
    }

    fn raw_append(&mut self, b: u8) -> bool {
        if self.raw.len() >= RAW_BUF_CAP {
            return false;
        }
        self.raw.push(b);
        true
    }

    /// Fold the open attempt (if any) into settled raw content.
    fn resolve_raw(&mut self) {
        self.raw_head = self.raw.len();
        self.state = State::Idle;
    }

    /// Like `resolve_raw`, but excludes the just-appended byte -- used when
    /// that byte is a SOF starting a *new* attempt, not part of the old
    /// one's settled span.
    fn resolve_raw_before_last(&mut self) {
        self.raw_head = self.raw.len() - 1;
        self.state = State::Idle;
    }

    fn checksum_ok(&self) -> bool {
        if checksum::is_ignore_sentinel(self.checksum_type, &self.checksum_bytes) {
            return true;
        }
        self.running.finalize() == self.checksum_bytes
    }

    /// Decode `raw[raw_head+2..data_end)` (unescaping) into a queued
    /// `Frame`, then discard the open attempt's raw bytes (they're now
    /// represented by the decoded frame instead of raw content).
    fn resolve_success(&mut self) {
        let type_byte = self.raw[self.raw_head + 1];
        let mut decoded = Vec::with_capacity(self.data_end.saturating_sub(self.raw_head + 2));
        let mut i = self.raw_head + 2;
        while i < self.data_end {
            let mut b = self.raw[i];
            i += 1;
            if b == ESC {
                b = unescape_byte(self.raw[i]);
                i += 1;
            }
            decoded.push(b);
        }
        self.frames.push_back(Frame { type_byte, data: decoded });
        self.raw.truncate(self.raw_head);
        self.state = State::PostFrame;
    }

    /// Feed one received byte. Returns the resulting status; drain ready
    /// content with [`get`](Decoder::get).
    pub fn inject(&mut self, ch: u8) -> InjectStatus {
        if !self.raw_append(ch) {
            self.resolve_raw();
            self.drop_count += 1;
            return self.status(false, true);
        }

        // \r\n trailer after a successful decode: display-only, consumed
        // here so it never pollutes raw recovery. Anything else falls
        // through to ordinary processing below, using the (now possibly
        // reset) state.
        if self.state == State::PostFrame {
            if ch == CR {
                self.raw.pop();
                self.state = State::PostFrameCr;
                return self.status(false, false);
            }
            self.state = State::Idle;
        }
        if self.state == State::PostFrameCr {
            if ch == LF {
                self.raw.pop();
                self.state = State::Idle;
                return self.status(false, false);
            }
            self.state = State::Idle;
        }

        if self.state == State::Checksum {
            self.checksum_bytes.push(ch);
            if self.checksum_bytes.len() < self.checksum_len {
                return self.status(false, false);
            }
            if self.checksum_ok() {
                self.resolve_success();
                return InjectStatus { frame_ready: true, checksum_error: false, buffer_full: false };
            }
            self.resolve_raw();
            return self.status(true, false);
        }

        // SOF always (re)starts an attempt, except mid-escape (a SOF-valued
        // byte immediately after ESC is just the escaped literal).
        if ch == SOF && self.state != State::DataEsc {
            self.resolve_raw_before_last();
            self.running = Running::new(self.checksum_type);
            self.running.update(SOF);
            self.data_len = 0;
            self.state = State::Sof;
            return self.status(false, false);
        }

        // TYPE byte right after SOF.
        if self.state == State::Sof {
            if ch == EOF {
                // SOF immediately followed by EOF: degenerate, not a frame.
                self.resolve_raw();
                return self.status(false, false);
            }
            self.running.update(ch);
            self.state = State::Data;
            return self.status(false, false);
        }

        if ch == ESC && self.state == State::Data {
            self.state = State::DataEsc;
            return self.status(false, false);
        }

        if ch == EOF && self.state != State::DataEsc {
            if self.state != State::Data && self.state != State::Eof {
                // A stray EOF with no open frame: settle quietly, no state
                // change (we're already Idle in practice).
                self.raw_head = self.raw.len();
                return self.status(false, false);
            }
            if self.state == State::Data {
                self.data_end = self.raw.len() - 1;
                self.running.update(EOF);
                if self.checksum_type == ChecksumType::Internet && self.data_len.is_multiple_of(2) {
                    self.state = State::Eof;
                    return self.status(false, false);
                }
            } else {
                // The expected second (padding) EOF.
                self.running.update(EOF);
            }
            self.checksum_bytes.clear();
            self.checksum_len = checksum::encoded_len(self.checksum_type);
            self.state = State::Checksum;
            return self.status(false, false);
        }

        // Expected the second padding EOF, got something else.
        if self.state == State::Eof {
            self.resolve_raw();
            return self.status(false, false);
        }

        // DATA_ESC: the next byte is always consumed as the escaped
        // literal, whatever its value -- an illegal escape target is not
        // rejected here, it just means the checksum won't match later.
        if self.state == State::DataEsc {
            let decoded = unescape_byte(ch);
            self.running.update(decoded);
            self.data_len += 1;
            self.state = State::Data;
            return self.status(false, false);
        }

        // Plain byte.
        if self.state != State::Data {
            self.raw_head = self.raw.len();
            return self.status(false, false);
        }
        self.running.update(ch);
        self.data_len += 1;
        self.status(false, false)
    }

    /// Recover a stalled open attempt as raw content after `timeout_ms` of
    /// no forward progress. Call periodically with a monotonic millisecond
    /// clock; this module never reads a clock itself.
    pub fn tick(&mut self, now_ms: u64) -> InjectStatus {
        let open_len = self.raw.len() - self.raw_head;
        if open_len != self.open_len_at_last_tick {
            self.open_len_at_last_tick = open_len;
            self.time_of_last_progress = now_ms;
        } else if open_len > 0 && now_ms.saturating_sub(self.time_of_last_progress) >= self.timeout_ms {
            self.resolve_raw();
        }
        self.status(false, false)
    }

    /// Pop the next ready frame (decoded, or recovered raw content with
    /// `type_byte == TYPE_RAW`), if any.
    pub fn get(&mut self) -> Option<Frame> {
        if let Some(f) = self.frames.pop_front() {
            return Some(f);
        }
        if self.raw_head > 0 {
            let data: Vec<u8> = self.raw.drain(0..self.raw_head).collect();
            self.raw_head = 0;
            return Some(Frame { type_byte: TYPE_RAW, data });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(checksum_type: ChecksumType, type_byte: u8, data: &[u8]) -> Frame {
        let wire = encode(checksum_type, type_byte, data);
        let mut dec = Decoder::new(checksum_type);
        let mut last = InjectStatus::default();
        for &b in &wire {
            last = dec.inject(b);
        }
        assert!(last.frame_ready, "expected a ready frame after full wire bytes");
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
        assert!(wire.len() > 2 + data.len(), "some escaping must have occurred");
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
        assert!(dec.get().is_none(), "trailer must not surface as raw content");
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
}
