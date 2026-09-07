// Copyright (c) 2026 Microchip Technology Inc. and its subsidiaries.
// SPDX-License-Identifier: MIT

use std::time::{Duration, Instant};

use mup1::{frame_type, ChecksumType, Mup1Client};

fn main() {
    let device = std::env::args().nth(1).unwrap_or_else(|| "/dev/ttyACM0".to_string());
    let transport = mup1::open_device(&device, 115200).expect("open device");
    let mut client = Mup1Client::new(transport, ChecksumType::Internet);

    println!("sending Ping to {device}...");
    client.send(frame_type::PING, &[]).expect("send ping");

    let start = Instant::now();
    let mut got_any = false;
    while start.elapsed() < Duration::from_secs(3) {
        client
            .poll(|f| {
                got_any = true;
                if f.type_byte == mup1::TYPE_RAW {
                    println!("RAW: {:?}", String::from_utf8_lossy(&f.data));
                } else {
                    println!(
                        "FRAME type={:?} ({:#04x}) data={:?} ({} bytes)",
                        f.type_byte as char,
                        f.type_byte,
                        String::from_utf8_lossy(&f.data),
                        f.data.len()
                    );
                }
            })
            .expect("poll");
        std::thread::sleep(Duration::from_millis(20));
    }
    if !got_any {
        println!("no response received within timeout");
        std::process::exit(1);
    }
}
