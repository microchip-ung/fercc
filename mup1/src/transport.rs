//! Byte-stream transports carrying MUP1 framing: a real serial device, or a
//! `termhub://`/`telnet://host:port` TCP bridge (mirrors
//! `support/libeasy/handler/dut.rb`'s `-d` URI-scheme dispatch, and matches
//! the `test/hil.c` termhub shim used by `client-lib`'s own HIL tests).

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// A byte stream carrying MUP1 frames, with a bounded read so polling loops
/// never block forever.
pub trait Transport: Read + Write {}

pub struct SerialTransport {
    port: Box<dyn serialport::SerialPort>,
}

impl SerialTransport {
    pub fn open(path: &str, baud_rate: u32) -> io::Result<Self> {
        let port = serialport::new(path, baud_rate)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(50))
            .open()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
        Ok(Self { port })
    }
}

impl Read for SerialTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.port.read(buf) {
            // A plain read timeout with nothing available is normal
            // polling, not an error.
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
            other => other,
        }
    }
}

impl Write for SerialTransport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.port.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.port.flush()
    }
}

impl Transport for SerialTransport {}

pub struct TcpTransport {
    stream: TcpStream,
}

impl TcpTransport {
    pub fn connect(host: &str, port: u16) -> io::Result<Self> {
        let stream = TcpStream::connect((host, port))?;
        stream.set_read_timeout(Some(Duration::from_millis(50)))?;
        stream.set_nodelay(true)?;
        Ok(Self { stream })
    }

    /// `telnet://` needs a short IAC negotiation before the line looks
    /// like a raw byte pipe (mirrors `dut.rb`'s telnet setup: WILL
    /// SUPPRESS-GO-AHEAD, DO SUPPRESS-GO-AHEAD, DO ECHO, then discard
    /// whatever the far end had buffered).
    pub fn connect_telnet(host: &str, port: u16) -> io::Result<Self> {
        let mut t = Self::connect(host, port)?;
        t.stream
            .write_all(&[0xff, 0xfb, 0x03, 0xff, 0xfd, 0x03, 0xff, 0xfd, 0x01])?;
        let mut discard = [0u8; 1024];
        let _ = t.stream.read(&mut discard);
        Ok(t)
    }
}

impl Read for TcpTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.stream.read(buf) {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut => Ok(0),
            other => other,
        }
    }
}

impl Write for TcpTransport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl Transport for TcpTransport {}

/// Open a device string exactly like the Ruby `-d`/`--device` option:
/// `termhub://host:port` or `telnet://host:port` for a TCP bridge, anything
/// else as a local serial device path.
pub fn open_device(device: &str, baud_rate: u32) -> io::Result<Box<dyn Transport>> {
    if let Some(rest) = device.strip_prefix("termhub://") {
        let (host, port) = split_host_port(rest)?;
        Ok(Box::new(TcpTransport::connect(host, port)?))
    } else if let Some(rest) = device.strip_prefix("telnet://") {
        let (host, port) = split_host_port(rest)?;
        Ok(Box::new(TcpTransport::connect_telnet(host, port)?))
    } else {
        Ok(Box::new(SerialTransport::open(device, baud_rate)?))
    }
}

fn split_host_port(s: &str) -> io::Result<(&str, u16)> {
    let (host, port_str) = s
        .rsplit_once(':')
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, format!("expected host:port, got {s:?}")))?;
    let port: u16 = port_str
        .parse()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, format!("invalid port in {s:?}")))?;
    Ok((host, port))
}
