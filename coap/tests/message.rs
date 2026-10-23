// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use coap::message::*;

const PAYLOAD_MARKER: u8 = 0xFF;

#[test]
fn encodes_a_simple_get_with_uri_path_and_query() {
    let block2 = Block {
        num: 0,
        more: false,
        szx: 4,
    };
    let wire = encode_request(&RequestParams {
        method: Method::Get,
        msg_id: 0x1234,
        token: 7,
        uri: "c?c=c",
        content_format: None,
        accept: None,
        block1: None,
        block2,
        payload: &[],
    });
    assert_eq!(wire[0], (1 << 6) | 1);
    assert_eq!(wire[1], Method::Get as u8);
    assert_eq!(&wire[2..4], &[0x12, 0x34]);
    assert_eq!(wire[4], 7);
    // Option 11 (Uri-Path "c"), delta=11, len=1
    assert_eq!(wire[5], (11 << 4) | 1);
    assert_eq!(wire[6], b'c');
    // Option 15 (Uri-Query "c=c"), delta=15-11=4, len=3
    assert_eq!(wire[7], (4 << 4) | 3);
    assert_eq!(&wire[8..11], b"c=c");
    // Option 23 (Block2), delta=23-15=8, len=1 (value 0<<4|0|4=4)
    assert_eq!(wire[11], (8 << 4) | 1);
    assert_eq!(wire[12], 4);
}

#[test]
fn round_trips_response_with_content_format_and_block2() {
    // Build a minimal ACK response by hand: ver=1 type=ACK(2) tkl=1,
    // code=2.05 Content, msgid=0xABCD, token=9, Content-Format=140,
    // Block2 num=1 more=0 szx=4, payload "hi".
    let mut msg = vec![(1 << 6) | (2 << 4) | 1, (2 << 5) | 5, 0xAB, 0xCD, 9];
    // Content-Format option 12, delta=12, len=2 (140 needs 2 bytes)
    msg.push((12 << 4) | 2);
    msg.extend_from_slice(&140u16.to_be_bytes());
    // Block2 option 23, delta=11, len=1, value = (1<<4)|0|4 = 0x14
    msg.push((11 << 4) | 1);
    msg.push(0x14);
    msg.push(PAYLOAD_MARKER);
    msg.extend_from_slice(b"hi");

    let parsed = parse(&msg).expect("parses");
    assert_eq!(parsed.msg_type, MsgType::Ack);
    assert_eq!(parsed.code, (2 << 5) | 5);
    assert_eq!(parsed.msg_id, 0xABCD);
    assert_eq!(parsed.content_format, Some(140));
    assert_eq!(
        parsed.block2,
        Some(Block {
            num: 1,
            more: false,
            szx: 4
        })
    );
    assert_eq!(parsed.payload, b"hi");
}
