//! CoAP (RFC 7252) client with RFC 7959 blockwise transfer, plus the
//! CORECONF (draft-ietf-core-comi) method/content-format conventions used
//! by `mup1cc`, layered over a [`mup1::Mup1Client`].

pub mod client;
pub mod message;

pub use client::{Client, CoapError, Response, DEFAULT_BLOCK_SIZE};
pub use message::{content_format, Method};

use mup1::Frame;

impl Client {
    /// GET `/c` (whole running+operational datastore, or a sub-tree via
    /// query params) -- response content-format is `YANG_DATA_CBOR`.
    pub fn get(&mut self, uri: &str, on_other: impl FnMut(Frame)) -> Result<Response, CoapError> {
        self.request(Method::Get, uri, None, None, &[], on_other)
    }

    /// PUT `/c` (whole-datastore replace) with a `YANG_DATA_CBOR` body.
    pub fn put(&mut self, uri: &str, payload: &[u8], on_other: impl FnMut(Frame)) -> Result<Response, CoapError> {
        self.request(Method::Put, uri, Some(content_format::YANG_DATA_CBOR), None, payload, on_other)
    }

    /// POST (RPC/action invocation) with a `YANG_INSTANCES_CBOR` body.
    pub fn post(&mut self, uri: &str, payload: &[u8], on_other: impl FnMut(Frame)) -> Result<Response, CoapError> {
        self.request(Method::Post, uri, Some(content_format::YANG_INSTANCES_CBOR), None, payload, on_other)
    }

    pub fn delete(&mut self, uri: &str, on_other: impl FnMut(Frame)) -> Result<Response, CoapError> {
        self.request(Method::Delete, uri, None, None, &[], on_other)
    }

    /// FETCH: request body is a CBOR sequence of instance-identifiers
    /// (`YANG_IDENTIFIERS_CBOR`); response is a CBOR sequence of
    /// `{instance-identifier: value}` maps (`YANG_INSTANCES_CBOR`).
    pub fn fetch(&mut self, uri: &str, payload: &[u8], on_other: impl FnMut(Frame)) -> Result<Response, CoapError> {
        self.request(Method::Fetch, uri, Some(content_format::YANG_IDENTIFIERS_CBOR), None, payload, on_other)
    }

    /// iPATCH: request body is a CBOR sequence of
    /// `{instance-identifier: value}` maps (`YANG_INSTANCES_CBOR`);
    /// response is empty on success, or the same shape describing an error
    /// on failure.
    pub fn ipatch(&mut self, uri: &str, payload: &[u8], on_other: impl FnMut(Frame)) -> Result<Response, CoapError> {
        self.request(Method::Ipatch, uri, Some(content_format::YANG_INSTANCES_CBOR), None, payload, on_other)
    }
}
