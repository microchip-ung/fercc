// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! `fercc`: a Rust CLI clone of `mup1cc`. See this repo's commit history
//! for the porting notes; `--log-*` and DTLS (`-k`) are intentionally
//! out of scope for this port.
//!
//! Structured as a library (this file) plus a thin `main.rs` so the
//! actual logic -- including `conv`'s format-conversion core, `convert`
//! below -- is unit-testable directly (`fercc/tests/`), without spawning
//! the built binary as a subprocess.

mod home;
pub mod io;
pub mod opts;
pub mod topology;

use std::io::{Read, Write};
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

pub fn run() -> Result<(), String> {
    let opts = Opts::parse();

    if let Some(command) = &opts.command {
        return match command {
            opts::Command::Conv(args) => run_conv(args),
            opts::Command::Schema(args) => run_schema(args),
        };
    }

    opts.validate_query()?;

    if opts.key.is_some() {
        return Err("DTLS (-k/--key) is not yet supported by this port".to_string());
    }

    let topology = topology::load();
    let device = opts
        .device
        .clone()
        .or_else(|| topology::device_from(&topology))
        .ok_or("No device given -- pass -d <device> (e.g. -d /dev/ttyACM0 or -d termhub://host:port), or reserve an Easytest topology")?;
    let method = opts.method.clone().ok_or("No method given -- pass -m <method> (one of: fetch, ipatch, get, put, post, delete)")?;

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
        load_downloaded_schema(&mut coap, opts.catalog_fetcher.as_deref(), opts.verbose)?
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

pub fn load_workspace_schema(verbose: bool) -> Result<Schema, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let repo_root = yang::catalog::find_repo_root(&cwd).ok_or(
        "Couldn't find a workspace catalog: no ancestor of the current directory has support/scripts/gen-cc-nodes.yaml. \
         Run this from inside a checkout that has one, or pass an explicit set of .yang/.sid files instead \
         (or --no-workspace, on the main device flow)",
    )?;
    if verbose {
        eprintln!("workspace root: {}", repo_root.display());
    }
    let files = yang::catalog::load_workspace(&repo_root).map_err(|e| e.to_string())?;
    yang::schema::build(&files.yang, &files.sid).map_err(|e| e.to_string())
}

fn load_downloaded_schema(coap: &mut CoapClient, catalog_fetcher: Option<&str>, verbose: bool) -> Result<Schema, String> {
    let checksum = fetch_yang_lib_checksum_from_dut(coap, verbose)?;
    if verbose {
        eprintln!("YANG Lib checksum in DUT: {checksum}");
    }
    let cache_dir = cache_base_dir()?.join(&checksum);
    // NOT "yang_schema": mup1cc uses exactly that name, as a *file*
    // holding its Marshal-dumped parsed schema, directly under this
    // same per-checksum cache directory (`PersistentYangSchema`,
    // support/yang-enc/yang-schema.rb). This cache root is shared with
    // that tool (same ~/.velocitydrive-yang-cache/<checksum>/
    // convention) even though what's cached here is different -- the
    // raw catalog files, never a parsed schema -- so this port's own
    // subdirectory must not collide with that name.
    let catalog_dir = cache_dir.join("catalog");
    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
    yang::catalog::download_and_extract(&checksum, &catalog_dir, catalog_fetcher, verbose).map_err(|e| e.to_string())?;
    let files = yang::catalog::load_from_dir(&catalog_dir).map_err(|e| e.to_string())?;
    yang::schema::build(&files.yang, &files.sid).map_err(|e| e.to_string())
}

fn cache_base_dir() -> Result<PathBuf, String> {
    let home = home::home_dir().ok_or("couldn't determine your home directory, so there's nowhere to put the downloaded-catalog cache")?;
    Ok(home.join(".velocitydrive-yang-cache"))
}

// ===========================================================================
// `conv`/`schema` subcommands, folding in `yang-enc`'s CLI
// (support/yang-enc/yang-enc.rb).
// ===========================================================================

/// Sort trailing positional arguments into `.yang`/`.sid`/everything-else
/// groups by extension, mirroring `yang-enc.rb:93-96` -- order on the
/// command line doesn't matter, only each argument's own suffix.
pub fn partition_catalog_files(files: &[String]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut yangs = Vec::new();
    let mut sids = Vec::new();
    let mut rest = Vec::new();
    for f in files {
        if f.ends_with(".yang") {
            yangs.push(f.clone());
        } else if f.ends_with(".sid") {
            sids.push(f.clone());
        } else {
            rest.push(f.clone());
        }
    }
    (yangs, sids, rest)
}

/// Build a schema straight from an explicit set of catalog files, no
/// workspace/download acquisition at all -- mirrors `generate_yang_schema`
/// (yang-schema.rb:27-30).
pub fn build_explicit_schema(yangs: &[String], sids: &[String]) -> Result<Schema, String> {
    let read_all = |paths: &[String]| -> Result<Vec<String>, String> {
        paths.iter().map(|p| std::fs::read_to_string(p).map_err(|e| format!("reading {p}: {e}"))).collect()
    };
    let yang_sources = read_all(yangs)?;
    let sid_sources = read_all(sids)?;
    yang::schema::build(&yang_sources, &sid_sources).map_err(|e| e.to_string())
}

fn read_data_file_or_stdin_text(data_file: Option<&str>) -> Result<String, String> {
    match data_file {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("reading {path}: {e}")),
        None => {
            let mut text = String::new();
            std::io::stdin().read_to_string(&mut text).map_err(|e| format!("reading stdin: {e}"))?;
            Ok(text)
        }
    }
}

fn read_data_file_or_stdin_bytes(data_file: Option<&str>) -> Result<Vec<u8>, String> {
    match data_file {
        Some(path) => std::fs::read(path).map_err(|e| format!("reading {path}: {e}")),
        None => {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes).map_err(|e| format!("reading stdin: {e}"))?;
            Ok(bytes)
        }
    }
}

/// `puts`-equivalent for a rendered JSON/YAML string: both renderers may
/// or may not already end in a newline, so normalize to exactly one
/// rather than risking a doubled-up blank final line (matches
/// `io::write_output`'s own STDOUT path).
fn print_normalized(rendered: &str) {
    print!("{}", rendered.trim_end_matches('\n'));
    println!();
}

pub fn content_format_of(s: &str) -> ContentFormat {
    match s {
        "fetch" => ContentFormat::Fetch,
        "ipatch" => ContentFormat::Ipatch,
        "post" => ContentFormat::Post,
        "put" => ContentFormat::Put,
        "get" => ContentFormat::Get,
        _ => ContentFormat::Yang,
    }
}

/// Already-read input for [`convert`]: which variant is expected depends
/// on `input_format` (`"cbor"` wants `Bytes`, `"yaml"`/`"json"` want
/// `Text`) -- passing the wrong one is a caller bug, not a data error, so
/// `convert` panics rather than returning a `Result` for that case.
pub enum ConvInput<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
}

/// What [`convert`] produced: `Bytes` for a `cbor` output (write these
/// raw, no trailing newline needed), `Text` for `yaml`/`json` (a
/// caller-facing consumer -- the CLI, or a test -- decides how much
/// whitespace normalization it wants on top).
#[derive(Debug, PartialEq, Eq)]
pub enum ConvOutput {
    Text(String),
    Bytes(Vec<u8>),
}

/// The core of `fercc conv`/`yang-enc conv`, decoupled from actual file/
/// STDIN/STDOUT I/O -- mirrors `main`'s `'conv'` dispatch (yang-enc.rb:
/// 145-168). Direct, subprocess-free unit testing lives in
/// `fercc/tests/conv_validation.rs`.
pub fn convert(schema: &Schema, input_format: &str, output_format: &str, cf: ContentFormat, continue_on_error: bool, input: ConvInput) -> Result<ConvOutput, String> {
    let is_sequence = matches!(cf, ContentFormat::Fetch | ContentFormat::Ipatch | ContentFormat::Post);
    let text = || match input {
        ConvInput::Text(t) => t,
        ConvInput::Bytes(_) => panic!("convert: {input_format:?} input needs ConvInput::Text, got Bytes"),
    };
    let bytes = || match input {
        ConvInput::Bytes(b) => b,
        ConvInput::Text(_) => panic!("convert: {input_format:?} input needs ConvInput::Bytes, got Text"),
    };

    match (input_format, output_format) {
        ("json", "cbor") | ("yaml", "cbor") => {
            let format = if input_format == "json" { io::Format::Json } else { io::Format::Yaml };
            let value = io::parse(text(), format)?;
            let cbor_bytes = if is_sequence {
                let items = value.as_array().ok_or("a fetch/ipatch/post payload must be a YAML/JSON list at the top level, not a single value")?;
                codec::json_seq_to_cbor(schema, items, cf, continue_on_error).map_err(|e| e.to_string())?
            } else {
                codec::json_to_cbor(schema, &value, cf, continue_on_error).map_err(|e| e.to_string())?
            };
            Ok(ConvOutput::Bytes(cbor_bytes))
        }
        ("cbor", "json") | ("cbor", "yaml") => {
            let decoded = if is_sequence {
                Json::Array(codec::cbor_seq_to_json(schema, bytes(), cf).map_err(|e| e.to_string())?)
            } else {
                codec::cbor_to_json(schema, bytes(), cf).map_err(|e| e.to_string())?
            };
            let format = if output_format == "json" { io::Format::Json } else { io::Format::Yaml };
            Ok(ConvOutput::Text(io::render(&decoded, format)?))
        }
        ("json", "yaml") | ("yaml", "yaml") => {
            let format = if input_format == "json" { io::Format::Json } else { io::Format::Yaml };
            let value = io::parse(text(), format)?;
            Ok(ConvOutput::Text(io::render(&value, io::Format::Yaml)?))
        }
        ("yaml", "json") | ("json", "json") => {
            let format = if input_format == "json" { io::Format::Json } else { io::Format::Yaml };
            let value = io::parse(text(), format)?;
            Ok(ConvOutput::Text(io::render(&value, io::Format::Json)?))
        }
        ("cbor", "cbor") => {
            let normalized = codec::normalize_cbor_seq(bytes()).map_err(|e| e.to_string())?;
            Ok(ConvOutput::Bytes(normalized))
        }
        (i, o) => Err(format!("unsupported conversion {i} -> {o}")),
    }
}

/// `yang-enc conv`: mirrors `main`'s `'conv'` branches (yang-enc.rb:
/// 100-126, 145-168). Always reads its data from a file argument or
/// STDIN and always writes to STDOUT, matching the reference -- `-i`/
/// `-o` here select an *encoding*, unlike the same flags on the flat
/// device flow above, which select a *file path*.
fn run_conv(args: &opts::ConvArgs) -> Result<(), String> {
    let (yangs, sids, rest) = partition_catalog_files(&args.files);
    if rest.len() > 1 {
        return Err(format!("expected at most one data file on the command line, got {}: {}", rest.len(), rest.join(", ")));
    }
    let data_file = rest.first().map(String::as_str);

    let schema = if yangs.is_empty() && sids.is_empty() {
        load_workspace_schema(false)?
    } else if !yangs.is_empty() && !sids.is_empty() {
        build_explicit_schema(&yangs, &sids)?
    } else {
        return Err("pass both .yang and .sid files together to use an explicit catalog (or neither, to use the workspace catalog) -- got only one kind".to_string());
    };

    let cf = content_format_of(&args.content);
    let input = if args.input_format == "cbor" {
        ConvInput::Bytes(&read_data_file_or_stdin_bytes(data_file)?)
    } else {
        ConvInput::Text(&read_data_file_or_stdin_text(data_file)?)
    };

    match convert(&schema, &args.input_format, &args.output_format, cf, args.continue_on_error, input)? {
        ConvOutput::Bytes(b) => std::io::stdout().write_all(&b).map_err(|e| e.to_string()),
        ConvOutput::Text(s) => {
            print_normalized(&s);
            Ok(())
        }
    }
}

/// `yang-enc schema`: mirrors `main`'s `'schema'` branches (yang-enc.rb:
/// 128-136, 170-171).
fn run_schema(args: &opts::SchemaArgs) -> Result<(), String> {
    let (yangs, sids, rest) = partition_catalog_files(&args.files);
    if !rest.is_empty() {
        return Err(format!("schema doesn't take a data file -- unexpected argument(s): {}", rest.join(", ")));
    }
    if !sids.is_empty() {
        return Err(".sid files aren't needed for a JSON Schema -- pass only .yang files".to_string());
    }
    let schema = if yangs.is_empty() { load_workspace_schema(false)? } else { build_explicit_schema(&yangs, &[])? };
    let value = yang::json_schema::to_json_schema(&schema, schema.root, "yang");
    print_normalized(&serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?);
    Ok(())
}

/// Mirrors `fetch_yang_lib_checksum_from_dut` (mup1cc:81-89): a FETCH
/// whose request payload is a *bare* SID (not a sequence -- the schema
/// isn't loaded yet, so this bypasses the codec entirely), returning the
/// 16-byte checksum as a lowercase hex string.
fn fetch_yang_lib_checksum_from_dut(coap: &mut CoapClient, verbose: bool) -> Result<String, String> {
    let req = cbor::to_vec(&cbor::Value::from(SID_CHECKSUM));

    let resp = coap.fetch("c", &req, |f| print_other_frame(&f)).map_err(|e| e.to_string())?;
    if resp.code_class != 2 || resp.code_detail != 5 {
        return Err(format!(
            "couldn't get the device's YANG-library checksum (got CoAP response {}.{:02} instead of the expected 2.05) -- is this really a VelocityDRIVE-SP device?",
            resp.code_class, resp.code_detail
        ));
    }
    let mut pos = 0;
    let value = cbor::from_slice(resp.payload.as_slice(), &mut pos).map_err(|e| format!("the device's checksum response could not be decoded as CBOR: {e}"))?;
    let map = value.as_map().ok_or("the device's checksum response was malformed (expected a CBOR map)")?;
    let (_, val) = map.first().ok_or("the device's checksum response was empty")?;
    let bytes = val.as_bytes().ok_or("the device's checksum response was malformed (expected raw bytes)")?;
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
        return Err(format!("-q/--query only works with fetch and get, not {method}"));
    }
    Ok(format!("{base}?{}", query.join("&")))
}

/// Mirrors `Mup1Con#rx` (mup1cc:24-34): print whatever
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
            let items = input_data.and_then(Json::as_array).ok_or("fetch needs a YAML/JSON list as input, not a single value")?;
            let items = normalize_fetch_request_items(items);
            let cbor_req = codec::json_seq_to_cbor(schema, &items, ContentFormat::Fetch, continue_on_error).map_err(|e| e.to_string())?;
            (coap.fetch(url, &cbor_req, on_other).map_err(|e| e.to_string())?, ContentFormat::Fetch)
        }
        "ipatch" => {
            let items = input_data.and_then(Json::as_array).ok_or("ipatch needs a YAML/JSON list as input, not a single value")?;
            let cbor_req = codec::json_seq_to_cbor(schema, items, ContentFormat::Ipatch, continue_on_error).map_err(|e| e.to_string())?;
            (coap.ipatch(url, &cbor_req, on_other).map_err(|e| e.to_string())?, ContentFormat::Ipatch)
        }
        "get" => (coap.get(url, on_other).map_err(|e| e.to_string())?, ContentFormat::Get),
        "put" => {
            let obj = input_data.ok_or("put needs a YAML/JSON object (key/value fields) as input")?;
            let cbor_req = codec::json_to_cbor(schema, obj, ContentFormat::Put, continue_on_error).map_err(|e| e.to_string())?;
            (coap.put(url, &cbor_req, on_other).map_err(|e| e.to_string())?, ContentFormat::Put)
        }
        "post" => {
            let items = input_data.and_then(Json::as_array).ok_or("post needs a YAML/JSON list as input, not a single value")?;
            let cbor_req = codec::json_seq_to_cbor(schema, items, ContentFormat::Post, continue_on_error).map_err(|e| e.to_string())?;
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
