// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! Topology-file device default, matching `support/scripts/mup1cc`'s
//! `TOPO`/`get_device` (mup1cc:64-150): look for
//! `.mscc-libeasy-topology.yaml` in the current directory, then
//! `$HOME`, and read `dut.terminal` as the device URL.

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct Dut {
    terminal: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Topology {
    #[serde(default)]
    dut: Dut,
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut out = vec![PathBuf::from(".mscc-libeasy-topology.yaml")];
    if let Ok(home) = std::env::var("HOME") {
        out.push(PathBuf::from(home).join(".mscc-libeasy-topology.yaml"));
    }
    out
}

/// Load the first topology file found (cwd, then `$HOME`), if any.
pub fn load() -> Option<Topology> {
    for path in candidate_paths() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            return serde_yaml_ng::from_str(&text).ok();
        }
    }
    None
}

/// The device URL from a loaded topology's `dut.terminal`, if present.
pub fn device_from(topology: &Option<Topology>) -> Option<String> {
    topology.as_ref()?.dut.terminal.clone()
}
