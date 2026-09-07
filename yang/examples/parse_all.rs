// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use std::fs;
use yang::parser::Stmt;

fn count(s: &Stmt, counts: &mut std::collections::HashMap<String, usize>) {
    *counts.entry(s.keyword.clone()).or_insert(0) += 1;
    for sub in &s.subs {
        count(sub, counts);
    }
}

fn main() {
    let dir = std::env::args().nth(1).expect("usage: parse_all <dir>");
    let mut total = 0;
    let mut failed = 0;
    let mut counts = std::collections::HashMap::new();
    let mut entries: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yang") {
            continue;
        }
        total += 1;
        let src = fs::read_to_string(&path).unwrap();
        match yang::parser::parse_module(&src) {
            Ok(m) => count(&m, &mut counts),
            Err(e) => {
                failed += 1;
                println!("FAIL {}: {}", path.display(), e);
            }
        }
    }
    println!("{}/{} parsed successfully", total - failed, total);
    for kw in ["container", "leaf", "list", "leaf-list", "choice", "case", "typedef", "grouping", "uses", "augment", "identity", "rpc", "action", "notification"] {
        println!("{:>14}: {}", kw, counts.get(kw).unwrap_or(&0));
    }
    if failed > 0 {
        std::process::exit(1);
    }
}
