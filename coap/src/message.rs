//! CoAP (RFC 7252) message encode/decode, plus the CORECONF-specific
//! method codes (FETCH/iPATCH) and content-formats used for YANG-SID CBOR.
//!
//! Ported from `client-lib/src/lm_coap.c` (`serialize_frame`/`coap_parse`),
//! the hardware-verified reference, cross-checked against
//! `support/libeasy/frame/coap.rb` and `docs/sw_reqs/coreconf.adoc`.

const VERSION: u8 = 1;

const OPT_URI_PATH: u16 = 11;
const OPT_CONTENT_FORMAT: u16 = 12;
const OPT_URI_QUERY: u16 = 15;
const OPT_ACCEPT: u16 = 17;
const OPT_BLOCK2: u16 = 23;
const OPT_BLOCK1: u16 = 27;

const PAYLOAD_MARKER: u8 = 0xFF;

/// CORECONF-specific content-format numbers (`docs/sw_reqs/coreconf.adoc`).
pub mod content_format {
    pub const YANG_DATA_CBOR: u16 = 140;
    pub const YANG_IDENTIFIERS_CBOR: u16 = 141;
    pub const YANG_INSTANCES_CBOR: u16 = 142;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgType {
    Con,
    Non,
    Ack,
    Rst,
}

impl MsgType {
    fn from_bits(b: u8) -> Self {
        match b & 0x3 {
            0 => MsgType::Con,
            1 => MsgType::Non,
            2 => MsgType::Ack,
            _ => MsgType::Rst,
        }
    }
}

/// CoAP/CORECONF methods (`RFC 7252` GET/POST/PUT/DELETE plus the
/// CORECONF-specific FETCH=5 (RFC 8132) and iPATCH=7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get = 1,
    Post = 2,
    Put = 3,
    Delete = 4,
    Fetch = 5,
    Ipatch = 7,
}

pub fn code_class(code: u8) -> u8 {
    code >> 5
}
pub fn code_detail(code: u8) -> u8 {
    code & 0x1F
}
pub const RESPONSE_CONTINUE: u8 = (2 << 5) | 31;

/// A Block1/Block2 option value (RFC 7959).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Block {
    pub num: u32,
    pub more: bool,
    pub szx: u8,
}

impl Block {
    pub fn size(self) -> u32 {
        1u32 << (self.szx as u32 + 4)
    }

    pub fn szx_for_size(block_size: u32) -> Option<u8> {
        match block_size {
            16 => Some(0),
            32 => Some(1),
            64 => Some(2),
            128 => Some(3),
            256 => Some(4),
            512 => Some(5),
            1024 => Some(6),
            _ => None,
        }
    }

    fn encode_value(self) -> u32 {
        (self.num << 4) | (if self.more { 1 << 3 } else { 0 }) | (self.szx as u32 & 7)
    }

    fn decode_value(raw: u32) -> Self {
        Block { szx: (raw & 7) as u8, more: (raw >> 3) & 1 != 0, num: raw >> 4 }
    }
}

/// Parameters for one outgoing CoAP request frame (one wire message; the
/// caller drives Block1/Block2 continuations across multiple calls).
pub struct RequestParams<'a> {
    pub method: Method,
    pub msg_id: u16,
    pub token: u8,
    /// e.g. `"c"` or `"c?c=c"` -- split on `?` for the query, then `/` for
    /// Uri-Path segments, matching `serialize_frame`'s URI handling.
    pub uri: &'a str,
    pub content_format: Option<u16>,
    pub accept: Option<u16>,
    /// `None` omits the Block1 option entirely (small, single-block
    /// payload) -- matches the reference only emitting Block1 when the
    /// payload needs more than one block, or this already is a
    /// continuation.
    pub block1: Option<Block>,
    /// Always emitted: every request offers Block2 so an oversized
    /// response (including error bodies) can be fragmented.
    pub block2: Block,
    /// Already-sliced payload for *this* block (empty for a bodyless
    /// request, or a Block2-only continuation with no request body).
    pub payload: &'a [u8],
}

fn opt_uint_encode(val: u32, out: &mut Vec<u8>) {
    if val == 0 {
        return;
    } else if val <= 0xFF {
        out.push(val as u8);
    } else if val <= 0xFFFF {
        out.extend_from_slice(&(val as u16).to_be_bytes());
    } else if val <= 0xFF_FFFF {
        out.push((val >> 16) as u8);
        out.extend_from_slice(&((val & 0xFFFF) as u16).to_be_bytes());
    } else {
        out.extend_from_slice(&val.to_be_bytes());
    }
}

fn opt_uint_decode(val: &[u8]) -> u32 {
    match val.len() {
        1 => val[0] as u32,
        2 => u16::from_be_bytes([val[0], val[1]]) as u32,
        3 => ((val[0] as u32) << 16) | (u16::from_be_bytes([val[1], val[2]]) as u32),
        4 => u32::from_be_bytes([val[0], val[1], val[2], val[3]]),
        _ => 0,
    }
}

struct Writer {
    buf: Vec<u8>,
    prev_opt: u16,
}

impl Writer {
    fn new() -> Self {
        Self { buf: Vec::new(), prev_opt: 0 }
    }

    fn opt_header(&mut self, delta: u16, length: u16) {
        let delta_nibble: u8 = if delta < 13 {
            delta as u8
        } else if delta < 269 {
            13
        } else {
            14
        };
        let length_nibble: u8 = if length < 13 {
            length as u8
        } else if length < 269 {
            13
        } else {
            14
        };
        self.buf.push((delta_nibble << 4) | length_nibble);
        match delta_nibble {
            13 => self.buf.push((delta - 13) as u8),
            14 => self.buf.extend_from_slice(&(delta - 269).to_be_bytes()),
            _ => {}
        }
        match length_nibble {
            13 => self.buf.push((length - 13) as u8),
            14 => self.buf.extend_from_slice(&(length - 269).to_be_bytes()),
            _ => {}
        }
    }

    fn opt(&mut self, opt_num: u16, val: &[u8]) {
        let delta = opt_num - self.prev_opt;
        self.opt_header(delta, val.len() as u16);
        self.buf.extend_from_slice(val);
        self.prev_opt = opt_num;
    }

    fn opt_uint(&mut self, opt_num: u16, val: u32) {
        let mut enc = Vec::new();
        opt_uint_encode(val, &mut enc);
        self.opt(opt_num, &enc);
    }

    fn opt_block(&mut self, opt_num: u16, block: Block) {
        let mut enc = Vec::new();
        opt_uint_encode(block.encode_value(), &mut enc);
        self.opt(opt_num, &enc);
    }
}

fn uri_segments(s: &str, delim: char) -> impl Iterator<Item = &str> {
    s.split(delim).filter(|seg| !seg.is_empty())
}

/// Encode one outgoing CoAP request frame.
pub fn encode_request(p: &RequestParams) -> Vec<u8> {
    let mut w = Writer::new();

    // ver(2) type(2) tkl(4); type is always Confirmable, tkl is always 1
    // (this client always sends a single-byte token) -- matches
    // serialize_frame's hardcoded header byte.
    w.buf.push((VERSION << 6) | (0u8 << 4) | 1u8);
    w.buf.push(p.method as u8);
    w.buf.extend_from_slice(&p.msg_id.to_be_bytes());
    w.buf.push(p.token);

    let (path, query) = match p.uri.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (p.uri, None),
    };
    let path = path.strip_prefix('/').unwrap_or(path);
    for seg in uri_segments(path, '/') {
        w.opt(OPT_URI_PATH, seg.as_bytes());
    }

    if let Some(cf) = p.content_format {
        w.opt_uint(OPT_CONTENT_FORMAT, cf as u32);
    }

    if let Some(query) = query {
        for seg in uri_segments(query, '&') {
            w.opt(OPT_URI_QUERY, seg.as_bytes());
        }
    }

    if let Some(accept) = p.accept {
        w.opt_uint(OPT_ACCEPT, accept as u32);
    }

    // Block2: always offered, `more` is meaningless on a request.
    w.opt_block(OPT_BLOCK2, Block { num: p.block2.num, more: false, szx: p.block2.szx });

    if let Some(block1) = p.block1 {
        w.opt_block(OPT_BLOCK1, block1);
    }

    if !p.payload.is_empty() {
        w.buf.push(PAYLOAD_MARKER);
        w.buf.extend_from_slice(p.payload);
    }

    w.buf
}

#[derive(Debug, Clone)]
pub struct ParsedMessage {
    pub msg_type: MsgType,
    pub code: u8,
    pub msg_id: u16,
    pub content_format: Option<u16>,
    pub block1: Option<Block>,
    pub block2: Option<Block>,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub struct ParseError;

/// Decode one received CoAP message (a response, in this client's usage).
pub fn parse(msg: &[u8]) -> Result<ParsedMessage, ParseError> {
    if msg.len() < 4 {
        return Err(ParseError);
    }
    let version = (msg[0] >> 6) & 3;
    if version != VERSION {
        return Err(ParseError);
    }
    let msg_type = MsgType::from_bits(msg[0] >> 4);
    let tkl = (msg[0] & 0x0F) as usize;
    let code = msg[1];
    let msg_id = u16::from_be_bytes([msg[2], msg[3]]);

    if tkl > 8 || 4 + tkl > msg.len() {
        return Err(ParseError);
    }

    let mut pos = 4 + tkl;
    let mut prev_opt_num: u16 = 0;
    let mut content_format = None;
    let mut block1 = None;
    let mut block2 = None;
    let mut payload = Vec::new();

    while pos < msg.len() {
        if msg[pos] == PAYLOAD_MARKER {
            pos += 1;
            if pos >= msg.len() {
                return Err(ParseError);
            }
            payload = msg[pos..].to_vec();
            return Ok(ParsedMessage { msg_type, code, msg_id, content_format, block1, block2, payload });
        }

        let opt_byte = msg[pos];
        pos += 1;
        let mut opt_delta = ((opt_byte >> 4) & 0x0F) as u16;
        let mut opt_len = (opt_byte & 0x0F) as u16;

        match opt_delta {
            13 => {
                if pos >= msg.len() {
                    return Err(ParseError);
                }
                opt_delta = msg[pos] as u16 + 13;
                pos += 1;
            }
            14 => {
                if pos + 1 >= msg.len() {
                    return Err(ParseError);
                }
                opt_delta = u16::from_be_bytes([msg[pos], msg[pos + 1]]) + 269;
                pos += 2;
            }
            15 => return Err(ParseError),
            _ => {}
        }

        match opt_len {
            13 => {
                if pos >= msg.len() {
                    return Err(ParseError);
                }
                opt_len = msg[pos] as u16 + 13;
                pos += 1;
            }
            14 => {
                if pos + 1 >= msg.len() {
                    return Err(ParseError);
                }
                opt_len = u16::from_be_bytes([msg[pos], msg[pos + 1]]) + 269;
                pos += 2;
            }
            15 => return Err(ParseError),
            _ => {}
        }

        let opt_len = opt_len as usize;
        if pos + opt_len > msg.len() {
            return Err(ParseError);
        }
        let opt_num = prev_opt_num + opt_delta;
        let val = &msg[pos..pos + opt_len];

        match opt_num {
            OPT_CONTENT_FORMAT => content_format = Some(opt_uint_decode(val) as u16),
            OPT_BLOCK2 => block2 = Some(Block::decode_value(opt_uint_decode(val))),
            OPT_BLOCK1 => block1 = Some(Block::decode_value(opt_uint_decode(val))),
            _ => {}
        }

        pos += opt_len;
        prev_opt_num = opt_num;
    }

    Ok(ParsedMessage { msg_type, code, msg_id, content_format, block1, block2, payload })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_simple_get_with_uri_path_and_query() {
        let block2 = Block { num: 0, more: false, szx: 4 };
        let wire = encode_request(&RequestParams {
            method: Method::Get,
            msg_id: 0x1234,
            token: 7,
            uri: "c?c=c",
            content_format: None,
            accept: None,
            block1: None,
            block2,
            payload: &[],
        });
        assert_eq!(wire[0], (1 << 6) | (0 << 4) | 1);
        assert_eq!(wire[1], Method::Get as u8);
        assert_eq!(&wire[2..4], &[0x12, 0x34]);
        assert_eq!(wire[4], 7);
        // Option 11 (Uri-Path "c"), delta=11, len=1
        assert_eq!(wire[5], (11 << 4) | 1);
        assert_eq!(wire[6], b'c');
        // Option 15 (Uri-Query "c=c"), delta=15-11=4, len=3
        assert_eq!(wire[7], (4 << 4) | 3);
        assert_eq!(&wire[8..11], b"c=c");
        // Option 23 (Block2), delta=23-15=8, len=1 (value 0<<4|0|4=4)
        assert_eq!(wire[11], (8 << 4) | 1);
        assert_eq!(wire[12], 4);
    }

    #[test]
    fn round_trips_response_with_content_format_and_block2() {
        // Build a minimal ACK response by hand: ver=1 type=ACK(2) tkl=1,
        // code=2.05 Content, msgid=0xABCD, token=9, Content-Format=140,
        // Block2 num=1 more=0 szx=4, payload "hi".
        let mut msg = vec![(1 << 6) | (2 << 4) | 1, (2 << 5) | 5, 0xAB, 0xCD, 9];
        // Content-Format option 12, delta=12, len=2 (140 needs 2 bytes)
        msg.push((12 << 4) | 2);
        msg.extend_from_slice(&140u16.to_be_bytes());
        // Block2 option 23, delta=11, len=1, value = (1<<4)|0|4 = 0x14
        msg.push((11 << 4) | 1);
        msg.push(0x14);
        msg.push(PAYLOAD_MARKER);
        msg.extend_from_slice(b"hi");

        let parsed = parse(&msg).expect("parses");
        assert_eq!(parsed.msg_type, MsgType::Ack);
        assert_eq!(parsed.code, (2 << 5) | 5);
        assert_eq!(parsed.msg_id, 0xABCD);
        assert_eq!(parsed.content_format, Some(140));
        assert_eq!(parsed.block2, Some(Block { num: 1, more: false, szx: 4 }));
        assert_eq!(parsed.payload, b"hi");
    }

    #[test]
    fn block_size_round_trips_through_szx() {
        for size in [16u32, 32, 64, 128, 256, 512, 1024] {
            let szx = Block::szx_for_size(size).unwrap();
            let block = Block { num: 3, more: true, szx };
            assert_eq!(block.size(), size);
            assert_eq!(Block::decode_value(block.encode_value()), block);
        }
    }
}
