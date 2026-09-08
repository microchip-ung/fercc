// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! Thin CLI entry point -- all real logic lives in `lib.rs`, so it's
//! directly unit-testable without spawning this binary as a subprocess.

fn main() {
    if let Err(e) = rcc::run() {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}
