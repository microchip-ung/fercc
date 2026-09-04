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
    let schema_dir = cache_dir.join("yang_schema");
    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
    yang::catalog::download_and_extract(&checksum, &schema_dir, verbose).map_err(|e| e.to_string())?;
    let files = yang::catalog::load_from_dir(&schema_dir).map_err(|e| e.to_string())?;
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

    let (resp, output): (Response, Option<Json>) = match method {
        "fetch" => {
            let items = input_data.and_then(Json::as_array).ok_or("fetch requires a YAML/JSON sequence as input")?;
            let cbor_req = codec::json_seq_to_cbor(schema, items, ContentFormat::Fetch).map_err(|e| annotate(e.to_string(), continue_on_error))?;
            let resp = coap.fetch(url, &cbor_req, on_other).map_err(|e| e.to_string())?;
            let output = codec::cbor_seq_to_json(schema, &resp.payload, ContentFormat::Fetch).map_err(|e| e.to_string())?;
            (resp, Some(Json::Array(output)))
        }
        "ipatch" => {
            let items = input_data.and_then(Json::as_array).ok_or("ipatch requires a YAML/JSON sequence as input")?;
            let cbor_req = codec::json_seq_to_cbor(schema, items, ContentFormat::Ipatch).map_err(|e| annotate(e.to_string(), continue_on_error))?;
            let resp = coap.ipatch(url, &cbor_req, on_other).map_err(|e| e.to_string())?;
            // A non-empty iPATCH response is documented as an error body
            // shaped as a whole-tree ("yang"/content-format 140) map, but
            // real hardware has also been observed echoing a successful
            // iPATCH's touched-instance confirmation as a content-format
            // 142 (instances) single-entry map -- decode according to
            // whichever content-format the response actually reports,
            // falling back to the raw bytes if that fails or the format
            // is unrecognized (this catalog has no SIDs registered for
            // the coreconf-error yang-data structure at all, so a genuine
            // error body may not resolve regardless).
            let output = decode_optional_response(schema, &resp);
            (resp, output)
        }
        "get" => {
            let resp = coap.get(url, on_other).map_err(|e| e.to_string())?;
            let output = codec::cbor_to_json(schema, &resp.payload, ContentFormat::Get).map_err(|e| e.to_string())?;
            (resp, Some(output))
        }
        "put" => {
            let obj = input_data.ok_or("put requires a YAML/JSON object as input")?;
            let cbor_req = codec::json_to_cbor(schema, obj, ContentFormat::Put).map_err(|e| annotate(e.to_string(), continue_on_error))?;
            let resp = coap.put(url, &cbor_req, on_other).map_err(|e| e.to_string())?;
            (resp, None)
        }
        "post" => {
            let items = input_data.and_then(Json::as_array).ok_or("post requires a YAML/JSON sequence as input")?;
            let cbor_req = codec::json_seq_to_cbor(schema, items, ContentFormat::Post).map_err(|e| annotate(e.to_string(), continue_on_error))?;
            let resp = coap.post(url, &cbor_req, on_other).map_err(|e| e.to_string())?;
            let output = if resp.payload.is_empty() {
                None
            } else {
                Some(Json::Array(codec::cbor_seq_to_json(schema, &resp.payload, ContentFormat::Post).map_err(|e| e.to_string())?))
            };
            (resp, output)
        }
        "delete" => {
            let resp = coap.delete(url, on_other).map_err(|e| e.to_string())?;
            (resp, None)
        }
        other => return Err(format!("CoAP method {other} not implemented!")),
    };

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
fn decode_optional_response(schema: &Schema, resp: &Response) -> Option<Json> {
    if resp.payload.is_empty() {
        return None;
    }
    let decoded = match resp.content_format {
        Some(140) => codec::cbor_to_json(schema, &resp.payload, ContentFormat::Yang).map(|v| vec![v]),
        Some(142) => codec::cbor_seq_to_json(schema, &resp.payload, ContentFormat::Ipatch),
        _ => codec::cbor_to_json(schema, &resp.payload, ContentFormat::Yang).map(|v| vec![v]),
    };
    match decoded {
        Ok(mut v) if v.len() == 1 => Some(v.remove(0)),
        Ok(v) => Some(Json::Array(v)),
        Err(_) => Some(Json::String(format!("<undecodable response payload, {} bytes>", resp.payload.len()))),
    }
}

fn annotate(msg: String, continue_on_error: bool) -> String {
    if continue_on_error {
        eprintln!("WARNING: {msg} (continuing, --continue given)");
    }
    msg
}
