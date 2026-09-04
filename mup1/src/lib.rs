//! MUP1 (Microchip UART Protocol 1) framing.
//!
//! See `checksum` for the two checksum algorithms and `frame` for the
//! byte-stuffing framing and decode state machine.

pub mod checksum;
pub mod client;
pub mod frame;
pub mod transport;

pub use checksum::ChecksumType;
pub use client::Mup1Client;
pub use frame::{Decoder, Frame, InjectStatus, MAX_DATA_SIZE, TYPE_RAW};
pub use transport::{open_device, Transport};

/// MUP1 frame type bytes (`docs/sw_reqs/mup1.adoc`). Each is independently
/// present depending on device configuration; a client should tolerate
/// receiving any of them.
pub mod frame_type {
    /// Device -> host, sent on boot (and in reply to Ping, as Pong).
    pub const ANNOUNCE: u8 = b'A';
    /// CoAP request/response, either direction.
    pub const COAP_LOWER: u8 = b'c';
    pub const COAP_UPPER: u8 = b'C';
    /// DTLS record, either direction (tunnels CoAP). Not implemented by
    /// this client; frames of this type are neither sent nor decrypted.
    pub const DTLS_LOWER: u8 = b'd';
    pub const DTLS_UPPER: u8 = b'D';
    /// Host -> device liveness probe; device must Pong within 1s.
    pub const PING: u8 = b'p';
    pub const PONG: u8 = b'P';
    /// Device -> host free-form trace/log text.
    pub const TRACE: u8 = b'T';
}
