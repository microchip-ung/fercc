// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

//! Cross-platform home-directory lookup: `$HOME` on Unix, or
//! `%USERPROFILE%` (falling back to `%HOMEDRIVE%%HOMEPATH%`, the older
//! pair some environments still set instead) on Windows.

use std::path::PathBuf;

#[cfg(not(windows))]
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(windows)]
pub fn home_dir() -> Option<PathBuf> {
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        return Some(PathBuf::from(profile));
    }
    let mut path = PathBuf::from(std::env::var_os("HOMEDRIVE")?);
    path.push(std::env::var_os("HOMEPATH")?);
    Some(path)
}
