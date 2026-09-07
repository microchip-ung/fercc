// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! `mup1cc`: a Rust CLI clone of `support/scripts/mup1cc`. See
//! `mup1cc-rs/rust-mup1cc.txt` and the `mup1cc-rs` commit history for the
//! porting notes; `--log-*` and DTLS (`-k`) are intentionally out of
//! scope for this port.

mod io;
mod opts;
mod topology;

use std::path::PathBuf;

use clap::Parser;
use coap::{Client as CoapClient, Response};
use mup1::{ChecksumType, Mup1Client};
use opts::Opts;
use serde_json::Value as Json;
use yang::codec::{self, ContentFormat};
use yang::schema::Schema;

/// `ietf-constrained-yang-library:yang-library/checksum` (mup1cc:78).
const SID_CHECKSUM: i64 = 29304;

fn main() {
    if let Err(e) = run() {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let opts = Opts::parse();
    opts.validate_query()?;

    if opts.key.is_some() {
        return Err("DTLS (-k/--key) is not yet supported by this port".to_string());
    }

    let topology = topology::load();
    let device = opts.device.clone().or_else(|| topology::device_from(&topology)).ok_or("Missing device!")?;
    let method = opts.method.clone().ok_or("Missing method!")?;

    let checksum_type = match opts.checksum_type.as_str() {
        "crc32" => ChecksumType::Crc32,
        _ => ChecksumType::Internet,
    };

    if opts.verbose {
        eprintln!("opts: {opts:?}");
        eprintln!("device: {device}");
    }

    let transport = mup1::open_device(&device, opts.baudrate).map_err(|e| format!("opening device {device}: {e}"))?;
    let mup1_client = Mup1Client::new(transport, checksum_type);
    let mut coap = CoapClient::new(mup1_client);

    let use_workspace = match opts.workspace_flag() {
        Some(v) => v,
        None => topology.is_some(),
    };

    let schema = if use_workspace {
        if opts.verbose {
            eprintln!("Use schema based on current workspace");
        }
        load_workspace_schema(opts.verbose)?
    } else {
        load_downloaded_schema(&mut coap, opts.verbose)?
    };

    let input_data = if matches!(method.as_str(), "fetch" | "ipatch" | "put" | "post") {
        let (value, format) = io::read_input(opts.input.as_deref(), opts.input_format.as_deref())?;
        if opts.verbose {
            eprintln!("input_format: {format:?}");
        }
        Some(value)
    } else {
        None
    };

    let url = url_with_query_params("c", &opts.query, &method)?;
    if opts.verbose {
        eprintln!("URL: {url}");
    }

    let output_data = coap_method_run(&mut coap, &schema, input_data.as_ref(), &url, &method, opts.continue_on_error, opts.verbose)?;

    if let Some(output) = output_data {
        io::write_output(&output, opts.output.as_deref(), opts.output_format.as_deref())?;
    }
    io::flush_stdout();
    Ok(())
}

fn load_workspace_schema(verbose: bool) -> Result<Schema, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let repo_root = yang::catalog::find_repo_root(&cwd)
        .ok_or("Could not find the repo root (support/scripts/gen-cc-nodes.yaml not found in any ancestor of the current directory)")?;
    if verbose {
        eprintln!("workspace root: {}", repo_root.display());
    }
    let files = yang::catalog::load_workspace(&repo_root).map_err(|e| e.to_string())?;
    yang::schema::build(&files.yang, &files.sid).map_err(|e| e.to_string())
}

fn load_downloaded_schema(coap: &mut CoapClient, verbose: bool) -> Result<Schema, String> {
    let checksum = fetch_yang_lib_checksum_from_dut(coap, verbose)?;
    if verbose {
        eprintln!("YANG Lib checksum in DUT: {checksum}");
    }
    let cache_dir = cache_base_dir()?.join(&checksum);
    // NOT "yang_schema": the real Ruby tool uses exactly that name, as a
    // *file* holding its Marshal-dumped parsed schema, directly under
    // this same per-checksum cache directory (`PersistentYangSchema`,
    // support/yang-enc/yang-schema.rb). This cache root is shared with
    // that tool (same ~/.velocitydrive-yang-cache/<checksum>/
    // convention) even though what's cached here is different -- the
    // raw catalog files, never a parsed schema (see rust-mup1cc.txt) --
    // so this port's own subdirectory must not collide with that name.
    let catalog_dir = cache_dir.join("catalog");
    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
    yang::catalog::download_and_extract(&checksum, &catalog_dir, verbose).map_err(|e| e.to_string())?;
    let files = yang::catalog::load_from_dir(&catalog_dir).map_err(|e| e.to_string())?;
    yang::schema::build(&files.yang, &files.sid).map_err(|e| e.to_string())
}

fn cache_base_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".velocitydrive-yang-cache"))
}

/// Mirrors `fetch_yang_lib_checksum_from_dut` (mup1cc:81-89): a FETCH
/// whose request payload is a *bare* SID (not a sequence -- the schema
/// isn't loaded yet, so this bypasses the codec entirely), returning the
/// 16-byte checksum as a lowercase hex string.
fn fetch_yang_lib_checksum_from_dut(coap: &mut CoapClient, verbose: bool) -> Result<String, String> {
    let mut req = Vec::new();
    ciborium::into_writer(&ciborium::Value::from(SID_CHECKSUM), &mut req).map_err(|e| e.to_string())?;

    let resp = coap.fetch("c", &req, |f| print_other_frame(&f)).map_err(|e| e.to_string())?;
    if resp.code_class != 2 || resp.code_detail != 5 {
        return Err(format!("Unable to get YANG checksum (response {}.{:02})", resp.code_class, resp.code_detail));
    }
    let value: ciborium::Value = ciborium::from_reader(resp.payload.as_slice()).map_err(|e| format!("decoding checksum response: {e}"))?;
    let map = value.as_map().ok_or("expected a CBOR map in the checksum response")?;
    let (_, val) = map.first().ok_or("empty checksum response")?;
    let bytes = val.as_bytes().ok_or("expected a byte-string checksum value")?;
    let hex = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if verbose {
        eprintln!("checksum bytes decoded ok");
    }
    Ok(hex)
}

fn url_with_query_params(base: &str, query: &[String], method: &str) -> Result<String, String> {
    if query.is_empty() {
        return Ok(base.to_string());
    }
    if method != "fetch" && method != "get" {
        return Err(format!("CoAP method {method} does not support query parameters!"));
    }
    Ok(format!("{base}?{}", query.join("&")))
}

/// Mirrors `Mup1Con#rx` (support/scripts/mup1cc:24-34): print whatever
/// non-CoAP traffic (console text, device trace, boot announce) arrives
/// while a request is in flight.
fn print_other_frame(f: &mup1::Frame) {
    match f.type_byte {
        mup1::TYPE_RAW => {
            let s = String::from_utf8_lossy(&f.data);
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                println!("CON: {trimmed:?}");
            }
        }
        t if t == mup1::frame_type::TRACE => println!("TRACE: {:?}", String::from_utf8_lossy(&f.data)),
        t if t == mup1::frame_type::ANNOUNCE => println!("ANNOUNCE: {:?}", String::from_utf8_lossy(&f.data)),
        _ => {}
    }
}

/// Mirrors `coap_method_run` (mup1cc:203-246): dispatch the CoAP method,
/// encode/decode via the SID-CBOR codec, and report a non-2.xx response
/// code the same way.
fn coap_method_run(
    coap: &mut CoapClient,
    schema: &Schema,
    input_data: Option<&Json>,
    url: &str,
    method: &str,
    continue_on_error: bool,
    verbose: bool,
) -> Result<Option<Json>, String> {
    let on_other = |f: mup1::Frame| print_other_frame(&f);

    // A response's body shape depends on whether the request actually
    // succeeded, not just on which method was sent: an error response
    // (4.xx/5.xx) is a whole-tree ("yang"/content-format 140) map
    // regardless of method, while a success response is shaped per the
    // method's own documented convention -- and real hardware has also
    // been observed echoing extra content-format-142 confirmations on
    // otherwise-bodiless successes (e.g. a successful iPATCH). So every
    // branch below decodes adaptively via `decode_response_payload`
    // rather than assuming its own success shape unconditionally.
    let (resp, success_format): (Response, ContentFormat) = match method {
        "fetch" => {
            let items = input_data.and_then(Json::as_array).ok_or("fetch requires a YAML/JSON sequence as input")?;
            let items = normalize_fetch_request_items(items);
            let cbor_req = codec::json_seq_to_cbor(schema, &items, ContentFormat::Fetch).map_err(|e| annotate(e.to_string(), continue_on_error))?;
            (coap.fetch(url, &cbor_req, on_other).map_err(|e| e.to_string())?, ContentFormat::Fetch)
        }
        "ipatch" => {
            let items = input_data.and_then(Json::as_array).ok_or("ipatch requires a YAML/JSON sequence as input")?;
            let cbor_req = codec::json_seq_to_cbor(schema, items, ContentFormat::Ipatch).map_err(|e| annotate(e.to_string(), continue_on_error))?;
            (coap.ipatch(url, &cbor_req, on_other).map_err(|e| e.to_string())?, ContentFormat::Ipatch)
        }
        "get" => (coap.get(url, on_other).map_err(|e| e.to_string())?, ContentFormat::Get),
        "put" => {
            let obj = input_data.ok_or("put requires a YAML/JSON object as input")?;
            let cbor_req = codec::json_to_cbor(schema, obj, ContentFormat::Put).map_err(|e| annotate(e.to_string(), continue_on_error))?;
            (coap.put(url, &cbor_req, on_other).map_err(|e| e.to_string())?, ContentFormat::Put)
        }
        "post" => {
            let items = input_data.and_then(Json::as_array).ok_or("post requires a YAML/JSON sequence as input")?;
            let cbor_req = codec::json_seq_to_cbor(schema, items, ContentFormat::Post).map_err(|e| annotate(e.to_string(), continue_on_error))?;
            (coap.post(url, &cbor_req, on_other).map_err(|e| e.to_string())?, ContentFormat::Post)
        }
        "delete" => (coap.delete(url, on_other).map_err(|e| e.to_string())?, ContentFormat::Yang),
        other => return Err(format!("CoAP method {other} not implemented!")),
    };
    let output = decode_response_payload(schema, &resp, success_format);

    if resp.code_class != 2 {
        println!("ERROR: response code {}.{:02}", resp.code_class, resp.code_detail);
    }
    if verbose {
        eprintln!("response: {}.{:02}", resp.code_class, resp.code_detail);
    }
    Ok(output)
}

/// Decode a response payload whose shape is inferred from its own
/// reported content-format rather than assumed from the request method --
/// see the call site in `coap_method_run`'s `"ipatch"` arm for why.
/// The codec accepts a single-key `{path: null}` map as an alternate
/// spelling of a bare `path` string for a FETCH request entry (matching
/// `validate_instance_entry!`'s tolerance of a nil-valued entry) -- but
/// real hardware rejects `{iid: null}` outright on the wire (4.00 Bad
/// Request, "Invalid CBOR payload"); only the bare-IID form is accepted
/// for "just this path, no value". So normalize that spelling away
/// before it ever reaches the codec, here at the point where we know
/// this is genuinely an outgoing *request* -- the codec itself must not
/// do this collapse unconditionally, since the exact same JSON shape can
/// also be a *decoded response* entry being re-encoded (e.g. in the
/// fixture round-trip tests), where the map form is correct as-is.
fn normalize_fetch_request_items(items: &[Json]) -> Vec<Json> {
    items
        .iter()
        .map(|item| match item.as_object() {
            Some(map) if map.len() == 1 => {
                let (path, val) = map.iter().next().unwrap();
                if val.is_null() {
                    Json::String(path.clone())
                } else {
                    item.clone()
                }
            }
            _ => item.clone(),
        })
        .collect()
}

/// Decode a response payload by trusting its *own* reported
/// content-format over the request method's documented convention.
/// Per the firmware source (`appl/src/cc/lma_cc.c`'s main request
/// dispatch): a successful PUT or iPATCH unconditionally clears the
/// response payload ("Make sure no payload is returned on a successful
/// PUT/IPATCH") -- there is no success shape for either to decode at
/// all, ever, not even an echo of the touched instance. A successful
/// POST is different: it legitimately sets content-format 142
/// (instances) and returns the created instance. And a 4.00 Bad
/// Request's body, when present, is `ietf-coreconf:error` (a
/// whole-tree, content-format 140, map) -- built only when
/// `resp_code == BAD_REQUEST` and an error tag was actually set; every
/// other outcome leaves the content-format unspecified and the payload
/// cleared. So: any payload arriving on a PUT/iPATCH success, or on any
/// non-2.xx code other than exactly 4.00, is not something this device
/// is ever supposed to send -- report it as raw bytes rather than
/// forcing it through a decode it was never defined to match. (The
/// Ruby reference doesn't draw either of these distinctions -- it
/// decodes any non-empty iPATCH response payload as `'yang'`
/// unconditionally, regardless of response code, `mup1cc:213-216` --
/// this port intentionally does not replicate that.)
fn decode_response_payload(schema: &Schema, resp: &Response, success_format: ContentFormat) -> Option<Json> {
    if resp.payload.is_empty() {
        return None;
    }
    let is_success = resp.code_class == 2;
    let is_bad_request = resp.code_class == 4 && resp.code_detail == 0;
    let always_bodiless_on_success = matches!(success_format, ContentFormat::Put | ContentFormat::Ipatch);
    if (is_success && always_bodiless_on_success) || (!is_success && !is_bad_request) {
        return Some(Json::String(format!(
            "<{} bytes, response code {}.{:02} has no defined body shape>",
            resp.payload.len(),
            resp.code_class,
            resp.code_detail
        )));
    }
    let effective_format = match resp.content_format {
        Some(140) => ContentFormat::Yang,
        // 141/142 (identifiers/instances) are both the same sequence-of-
        // single-entry-maps shape on a response; ContentFormat::Fetch's
        // decode is the most permissive (also tolerates a bare IID item).
        Some(141) | Some(142) => ContentFormat::Fetch,
        _ => success_format,
    };
    let is_sequence = matches!(effective_format, ContentFormat::Fetch | ContentFormat::Ipatch | ContentFormat::Post);
    let decoded = if is_sequence {
        codec::cbor_seq_to_json(schema, &resp.payload, effective_format)
    } else {
        codec::cbor_to_json(schema, &resp.payload, effective_format).map(|v| vec![v])
    };
    match decoded {
        Ok(mut v) if v.len() == 1 => Some(v.remove(0)),
        Ok(v) => Some(Json::Array(v)),
        // Never hard-fail the whole command on a decode error -- one
        // unexpected/unresolvable field shouldn't crash an otherwise-
        // meaningful response -- but the underlying reason is real
        // diagnostic information (e.g. "unknown SID N" usually means
        // the loaded YANG catalog doesn't match the device's firmware
        // version), so report it rather than discarding it.
        Err(e) => Some(Json::String(format!("<could not decode {}-byte response payload: {e}>", resp.payload.len()))),
    }
}

fn annotate(msg: String, continue_on_error: bool) -> String {
    if continue_on_error {
        eprintln!("WARNING: {msg} (continuing, --continue given)");
    }
    msg
}
