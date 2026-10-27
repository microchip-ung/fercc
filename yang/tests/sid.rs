// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use yang::sid::*;

#[test]
fn parses_a_minimal_sid_file() {
    let src = r#"{
        "module-name": "board",
        "module-revision": "2026-03-31",
        "items": [
            {"namespace": "module", "identifier": "board", "status": "unstable", "sid": 38500},
            {"namespace": "data", "identifier": "/board:capabilities", "status": "unstable", "sid": 38501}
        ]
    }"#;
    let f = parse(src).unwrap();
    assert_eq!(f.module_name, "board");
    assert_eq!(f.items.len(), 2);
    assert_eq!(f.items[0].namespace, Namespace::Module);
    assert_eq!(f.items[1].identifier, "/board:capabilities");
    assert_eq!(f.items[1].sid, 38501);
}
