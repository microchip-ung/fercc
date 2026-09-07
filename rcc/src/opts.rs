// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! CLI option surface, matching `mup1cc`'s `OptionParser` block
//! flag-for-flag (minus `--log-append`/`--log-msg`/`--log-run`/
//! `--log-steps`, out of scope for this port).

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "rcc", about = "MUP1/CoAP/CORECONF client for VelocityDRIVE-SP devices")]
pub struct Opts {
    /// IP based terminal device to connect to. Ex: termhub://10.0.0.2:4000
    /// or /dev/ttyUSB0. If an Easytest setup is reserved then this
    /// defaults to the terminal specified in the topology file.
    #[arg(short = 'd', long = "device")]
    pub device: Option<String>,

    /// Use DTLS with this key file. Not yet supported by this port; any
    /// value here is rejected with a clear error at startup.
    #[arg(short = 'k', long = "key", value_name = "KEY_FILE.pem")]
    pub key: Option<String>,

    /// MUP1 checksum type: internet-checksum (default) or crc32.
    #[arg(long = "checksum-type", value_parser = ["internet-checksum", "crc32"], default_value = "internet-checksum")]
    pub checksum_type: String,

    /// Serial baud rate.
    #[arg(long = "baudrate", default_value_t = 115200)]
    pub baudrate: u32,

    /// CoAP method to process.
    #[arg(short = 'm', long = "method", value_parser = ["fetch", "ipatch", "get", "put", "post", "delete"])]
    pub method: Option<String>,

    /// Read the request from a file. Default is read from STDIN.
    #[arg(short = 'i', long = "input")]
    pub input: Option<String>,

    /// Write the response to a file. Default is to write to STDOUT.
    #[arg(short = 'o', long = "output")]
    pub output: Option<String>,

    /// Force a given encoding of input. Default is YAML, next is the
    /// extension of the file in -i, final is this argument.
    #[arg(short = 'I', long = "input-format", value_parser = ["json", "yaml"])]
    pub input_format: Option<String>,

    /// Force a given encoding of output. Default is YAML, next is the
    /// extension of the file in -o, final is this argument.
    #[arg(short = 'O', long = "output-format", value_parser = ["json", "yaml"])]
    pub output_format: Option<String>,

    /// Query parameters to get and fetch. Repeatable; c=* and d=*
    /// parameters are mutually exclusive within their own group.
    #[arg(short = 'q', long = "query", value_parser = ["c=c", "c=n", "c=a", "d=a", "d=t"])]
    pub query: Vec<String>,

    /// Not implemented by this port: accepted only for CLI-surface
    /// compatibility with the Ruby reference's OptionParser flag set.
    /// The Ruby tool uses this to set a trace level for its internal
    /// MUP1/CoAP/DTLS handler-subsystem event logging; this port has no
    /// such internal event-tracer, so any value given here is parsed
    /// and silently ignored.
    #[arg(short = 's', long = "sys-trace", value_parser = ["fatal", "error", "info", "debug"])]
    pub sys_trace: Option<String>,

    /// Be more verbose.
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,

    /// Continue on errors in JSON schema validation.
    #[arg(short = 'c', long = "continue")]
    pub continue_on_error: bool,

    /// Use the YANG catalog from the current workspace.
    #[arg(short = 'w', long = "workspace", conflicts_with = "no_workspace")]
    pub workspace: bool,

    /// Always use the YANG catalog based on the current software in the
    /// DUT, even if an Easytest setup is reserved.
    #[arg(long = "no-workspace")]
    pub no_workspace: bool,
}

impl Opts {
    /// `None` (unset) / `Some(true)` / `Some(false)`, matching the Ruby
    /// tri-state `$opts[:workspace]`.
    pub fn workspace_flag(&self) -> Option<bool> {
        if self.workspace {
            Some(true)
        } else if self.no_workspace {
            Some(false)
        } else {
            None
        }
    }

    /// Duplicate-group check matching `mup1cc:419-425`: at most one
    /// `c=*` and one `d=*` query parameter.
    pub fn validate_query(&self) -> Result<(), String> {
        let mut seen = Vec::new();
        for q in &self.query {
            let group = &q[..2.min(q.len())];
            if seen.contains(&group) {
                return Err(format!("Duplicate query parameter {q}"));
            }
            seen.push(group);
        }
        Ok(())
    }
}
