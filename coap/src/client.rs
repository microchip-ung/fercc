//! Synchronous CoAP request/response driver with RFC 7959 blockwise
//! transfer, layered on a [`mup1::Mup1Client`].
//!
//! Ported from `client-lib/src/lm_coap.c`'s session state machine
//! (`lm_coap_request`/`process_response`/`lm_coap_tick`), the
//! hardware-verified reference, cross-checked against
//! `support/libeasy/handler/coap.rb`'s `ReqBlockWise`. Where the Ruby
//! reference builds a generic multi-handler poll loop, this mirrors the
//! simpler, single-session C state machine instead -- this client only
//! ever has one request in flight, exactly like both references.

use std::time::{Duration, Instant};

use mup1::{frame_type, Frame, Mup1Client};

use crate::message::{self, Block, Method, MsgType, ParsedMessage, RequestParams, RESPONSE_CONTINUE};

/// Default CoAP block size in bytes (SZX=4), matching
/// `Et::Handler::Coap::DEFAULT_BLOCK_SIZE` and `COAP_BLOCK_SZX` in
/// `lm_coap.c`.
pub const DEFAULT_BLOCK_SIZE: u32 = 256;

const RETRANSMIT_MS: u64 = 3000;
const MAX_RETRANSMIT: u32 = 5;

#[derive(Debug)]
pub struct Response {
    pub code_class: u8,
    pub code_detail: u8,
    pub content_format: Option<u16>,
    pub payload: Vec<u8>,
}

impl Response {
    pub fn is_success(&self) -> bool {
        self.code_class == 2
    }
}

#[derive(Debug)]
pub enum CoapError {
    Timeout,
    Io(std::io::Error),
    Protocol(&'static str),
}

impl From<std::io::Error> for CoapError {
    fn from(e: std::io::Error) -> Self {
        CoapError::Io(e)
    }
}

impl std::fmt::Display for CoapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoapError::Timeout => write!(f, "CoAP request timed out (no ACK after {MAX_RETRANSMIT} retransmits)"),
            CoapError::Io(e) => write!(f, "I/O error: {e}"),
            CoapError::Protocol(msg) => write!(f, "protocol error: {msg}"),
        }
    }
}

struct Session {
    block1_num: u32,
    block1_szx: u8,
    block2_num: u32,
    block2_szx: u8,
    msg_id: u16,
    /// Once a Block2 continuation happens, the original request body (if
    /// any) is never resent -- a block2-continuation request is a bare
    /// "send me the next chunk", like a GET (matches `process_response`
    /// clearing `session->req_payload`).
    req_payload_cleared: bool,
    resp_payload: Vec<u8>,
    resp_content_format: Option<u16>,
    resp_code: u8,
    req_payload_size: usize,
    done: bool,
    error: Option<CoapError>,
    need_resend: bool,
}

fn process_response(session: &mut Session, parsed: &ParsedMessage) {
    session.resp_code = parsed.code;
    session.resp_content_format = parsed.content_format;

    if !parsed.payload.is_empty() {
        session.resp_payload.extend_from_slice(&parsed.payload);
    }

    if let Some(b2) = parsed.block2 {
        if b2.more {
            session.block2_num = b2.num + 1;
            session.block2_szx = b2.szx;
            session.msg_id = session.msg_id.wrapping_add(1);
            session.req_payload_cleared = true;
            session.need_resend = true;
            return;
        }
    }

    if parsed.code == RESPONSE_CONTINUE {
        if let Some(b1) = parsed.block1 {
            let next_block_size = 1u32 << (b1.szx as u32 + 4);
            let next_offset = (session.block1_num as u64 + 1) * next_block_size as u64;
            if next_offset as usize >= session.req_payload_size {
                session.error = Some(CoapError::Protocol(
                    "server sent a spurious 2.31 Continue past the end of the request body",
                ));
                session.done = true;
                return;
            }
            session.block1_szx = b1.szx;
            session.block1_num += 1;
            session.msg_id = session.msg_id.wrapping_add(1);
            session.need_resend = true;
            return;
        }
    }

    session.done = true;
}

pub struct Client {
    mup1: Mup1Client,
    msg_id: u16,
    token: u8,
    block_size: u32,
}

impl Client {
    pub fn new(mup1: Mup1Client) -> Self {
        // Seeded from the wall clock + PID (not cryptographic, just
        // "different enough across processes") rather than starting at a
        // fixed 0, matching the reference's random per-request msg_id
        // (`Et::Handler::Coap`, `rand(2**16)`): every `mup1cc` invocation
        // is a short-lived fresh process, so a fixed starting msg_id
        // would let a slow/retransmitted reply to one invocation's
        // request spuriously match a *later* invocation's first request
        // of the same value.
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
            ^ (std::process::id() as u128);
        Self { mup1, msg_id: seed as u16, token: (seed >> 16) as u8, block_size: DEFAULT_BLOCK_SIZE }
    }

    /// Must be a power of two between 16 and 1024 (RFC 7959 SZX 0-6).
    pub fn set_block_size(&mut self, size: u32) -> Result<(), CoapError> {
        Block::szx_for_size(size).ok_or(CoapError::Protocol("block size must be a power of two, 16..=1024"))?;
        self.block_size = size;
        Ok(())
    }

    fn send_frame(
        &mut self,
        session: &Session,
        method: Method,
        uri: &str,
        content_format: Option<u16>,
        accept: Option<u16>,
        req_payload: &[u8],
    ) -> std::io::Result<()> {
        let effective_payload: &[u8] = if session.req_payload_cleared { &[] } else { req_payload };
        let block1_block_size = 1u32 << (session.block1_szx as u32 + 4);
        let mut block1_opt = None;
        let mut chunk: &[u8] = &[];

        if !effective_payload.is_empty() {
            let offset = (session.block1_num * block1_block_size) as usize;
            let remaining = effective_payload.len().saturating_sub(offset);
            let take = remaining.min(block1_block_size as usize);
            chunk = &effective_payload[offset..offset + take];

            if effective_payload.len() > block1_block_size as usize || session.block1_num > 0 {
                block1_opt = Some(Block {
                    num: session.block1_num,
                    more: remaining > block1_block_size as usize,
                    szx: session.block1_szx,
                });
            }
        }

        let params = RequestParams {
            method,
            msg_id: session.msg_id,
            token: self.token,
            uri,
            content_format,
            accept,
            block1: block1_opt,
            block2: Block { num: session.block2_num, more: false, szx: session.block2_szx },
            payload: chunk,
        };
        let wire = message::encode_request(&params);
        self.mup1.send(frame_type::COAP_LOWER, &wire)
    }

    /// Issue one CoAP/CORECONF request, driving Block1 upload and Block2
    /// download continuations to completion, retransmitting the current
    /// frame every 3s up to 5 times before giving up.
    ///
    /// `on_other_frame` receives every non-CoAP frame seen while waiting
    /// (Announce/Trace/Ping-Pong/raw console passthrough) -- mirrors
    /// `Mup1Con` in the Ruby CLI.
    pub fn request(
        &mut self,
        method: Method,
        uri: &str,
        content_format: Option<u16>,
        accept: Option<u16>,
        req_payload: &[u8],
        mut on_other_frame: impl FnMut(Frame),
    ) -> Result<Response, CoapError> {
        self.token = self.token.wrapping_add(1);
        self.msg_id = self.msg_id.wrapping_add(1);
        let szx = Block::szx_for_size(self.block_size).expect("validated by set_block_size");

        let mut session = Session {
            block1_num: 0,
            block1_szx: szx,
            block2_num: 0,
            block2_szx: szx,
            msg_id: self.msg_id,
            req_payload_cleared: false,
            resp_payload: Vec::new(),
            resp_content_format: None,
            resp_code: 0,
            req_payload_size: req_payload.len(),
            done: false,
            error: None,
            need_resend: false,
        };

        self.send_frame(&session, method, uri, content_format, accept, req_payload)?;
        let mut retransmit_deadline = Instant::now() + Duration::from_millis(RETRANSMIT_MS);
        let mut retransmit_count = 0u32;

        while !session.done {
            {
                let session_ref = &mut session;
                self.mup1.poll(|f| match f.type_byte {
                    t if t == frame_type::COAP_LOWER || t == frame_type::COAP_UPPER => {
                        let Ok(parsed) = message::parse(&f.data) else { return };
                        if parsed.msg_id != session_ref.msg_id {
                            return; // stale/duplicate reply for a prior frame in this exchange
                        }
                        match parsed.msg_type {
                            MsgType::Rst => {
                                session_ref.error = Some(CoapError::Protocol("device sent RST"));
                                session_ref.done = true;
                            }
                            MsgType::Ack => process_response(session_ref, &parsed),
                            _ => {} // unexpected type for a reply; ignore
                        }
                    }
                    _ => on_other_frame(f),
                })?;
            }

            if session.error.is_some() {
                break;
            }
            if session.need_resend {
                session.need_resend = false;
                retransmit_count = 0;
                retransmit_deadline = Instant::now() + Duration::from_millis(RETRANSMIT_MS);
                self.send_frame(&session, method, uri, content_format, accept, req_payload)?;
                continue;
            }
            if session.done {
                break;
            }
            if Instant::now() >= retransmit_deadline {
                if retransmit_count >= MAX_RETRANSMIT {
                    self.msg_id = session.msg_id;
                    return Err(CoapError::Timeout);
                }
                retransmit_count += 1;
                retransmit_deadline = Instant::now() + Duration::from_millis(RETRANSMIT_MS);
                self.send_frame(&session, method, uri, content_format, accept, req_payload)?;
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        self.msg_id = session.msg_id;
        if let Some(e) = session.error {
            return Err(e);
        }
        Ok(Response {
            code_class: message::code_class(session.resp_code),
            code_detail: message::code_detail(session.resp_code),
            content_format: session.resp_content_format,
            payload: session.resp_payload,
        })
    }
}
