//! Ties [`frame::Decoder`] to a [`transport::Transport`]: send frames,
//! poll for received ones. Mirrors what `Et::Handler::Mup1` +
//! `Et::Handler::Base`'s poll loop do together in the Ruby reference, minus
//! the generic multi-handler broadcast machinery -- here each frame type is
//! just handed to the caller to route.

use std::io;
use std::time::Instant;

use crate::checksum::ChecksumType;
use crate::frame::{self, Decoder, Frame};
use crate::transport::Transport;

pub struct Mup1Client {
    transport: Box<dyn Transport>,
    decoder: Decoder,
    checksum_type: ChecksumType,
    clock: Instant,
}

impl Mup1Client {
    pub fn new(transport: Box<dyn Transport>, checksum_type: ChecksumType) -> Self {
        Self {
            transport,
            decoder: Decoder::new(checksum_type),
            checksum_type,
            clock: Instant::now(),
        }
    }

    fn now_ms(&self) -> u64 {
        self.clock.elapsed().as_millis() as u64
    }

    /// Encode and transmit one frame.
    pub fn send(&mut self, type_byte: u8, data: &[u8]) -> io::Result<()> {
        let wire = frame::encode(self.checksum_type, type_byte, data);
        self.transport.write_all(&wire)
    }

    /// Read whatever is currently available (bounded by the transport's own
    /// read timeout, so this never blocks indefinitely), feed it to the
    /// decoder, and hand every newly-ready frame to `on_frame` in order.
    /// Returns `true` if at least one byte was read.
    pub fn poll(&mut self, mut on_frame: impl FnMut(Frame)) -> io::Result<bool> {
        let mut buf = [0u8; 1024];
        let n = self.transport.read(&mut buf)?;
        for &b in &buf[..n] {
            self.decoder.inject(b);
        }
        self.decoder.tick(self.now_ms());
        while let Some(f) = self.decoder.get() {
            on_frame(f);
        }
        Ok(n > 0)
    }
}
