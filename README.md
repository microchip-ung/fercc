# mup1cc-rs

A Rust port of `support/scripts/mup1cc`: a MUP1/CoAP/CORECONF client for
VelocityDRIVE-SP devices. See `rust-mup1cc.txt` for the original task
brief.

`support/scripts/mup1cc` ([also on GitHub](https://github.com/microchip-ung/velocitydrivesp-support/blob/main/support/scripts/mup1cc))
is the reference implementation. Any behavioral discrepancy between this
port and that tool is most likely a bug in this port, not in the
reference.

Status as of this writing: **core CLI parity implemented and verified
against real hardware** (`/dev/ttyACM0`), including a byte-for-byte
comparison of `mup1cc -m get -w` output against the real Ruby tool run
against the same board (see git log for the diff session), and **all 387
of the codec conformance fixtures pass byte-exact** (`cargo test`).
DTLS, `--log-*`, and a handful of smaller gaps are intentionally out of
scope for this pass -- see "Known gaps" below.

## Building

```
cargo build --workspace            # debug
cargo build --workspace --release  # optimized
cargo test --workspace             # unit tests + the 387-fixture codec
                                    # conformance suite (yang/tests/fixtures.rs)
```

Requires a stable Rust toolchain (developed against 1.98.1; no nightly
features used). No system packages needed -- `serialport` is built with
`default-features = false` (no libudev) since the device path is always
given explicitly.

## Running

```
cargo run -p mup1cc -- -d /dev/ttyACM0 -m get -w
echo '- "/ietf-system:system/contact"' | cargo run -p mup1cc -- -d /dev/ttyACM0 -m fetch -w
```

Flags mirror `support/scripts/mup1cc`'s `OptionParser` block -- see
`mup1cc/src/opts.rs` or `mup1cc --help`.

## Workspace layout

- `mup1/` -- MUP1 framing (checksums, byte-stuffing, decode state
  machine) + serial/TCP transport. Ported from `client-lib/src/lm_mup1.c`
  (the hardware-verified embedded reference), cross-checked against
  `support/libeasy/handler/mup1.rb`.
- `coap/` -- CoAP client (RFC 7252) with RFC 7959 blockwise transfer.
  Ported from `client-lib/src/lm_coap.c`, cross-checked against
  `support/libeasy/{handler/coap.rb,frame/coap.rb}`.
- `yang/` -- the big one:
  - `parser.rs` -- from-scratch RFC 7950 YANG tokenizer/parser (the Ruby
    reference has none -- it shells out to `pyang`; structurally modeled
    on `~/sw-velocitydrive-devclient`'s `yang-parser.ts`).
  - `schema.rs` -- semantic interpretation (groupings/uses, augment,
    deviation, identities, leafref resolution, choice/case flattening),
    porting `support/yang-enc/yang-utils.rb`'s semantics.
  - `sid.rs` -- RFC 9595 `.sid` file parsing.
  - `codec.rs` -- the SID-CBOR wire codec, porting
    `support/yang-enc/yang-enc.rb`.
  - `catalog.rs` -- workspace (`support/scripts/gen-cc-nodes.yaml`) and
    checksum-download catalog acquisition, porting
    `support/yang-enc/yang-schema.rb` minus its on-disk parsed-schema
    cache (re-parsing fresh every invocation is the explicit point of
    doing this in Rust -- see `rust-mup1cc.txt`).
- `mup1cc/` -- the CLI binary tying it all together.
- `test-data/` -- ~387 YAML/CBOR fixture pairs plus a bundled YANG
  catalog tarball, copied from `~/sw-velocitydrive-devclient`'s
  `test-data/` (a prior TypeScript port of this same stack). This is the
  primary codec conformance corpus (`yang/tests/fixtures.rs`), alongside
  `support/yang-enc/spec/tests/` in the main Ruby codebase.

## Known gaps

- **DTLS (`-k`)**: not implemented. The flag parses but errors clearly at
  startup if given a value.
- **`--log-append`/`--log-msg`/`--log-run`/`--log-steps`**: not
  implemented (out of scope per the task brief).
- **`-s`/`--sys-trace`**: not implemented. Accepted only for CLI-surface
  compatibility with the Ruby reference (so a script passing it doesn't
  break); any value given is silently ignored. The Ruby flag controls
  verbosity of internal handler-subsystem event logging, which this
  port has no equivalent of. This is unrelated to device-emitted MUP1
  trace frames, which are handled unconditionally regardless of `-s`
  and have been verified against real hardware.
- **Validation**: this port does not replicate the Ruby reference's
  JSON-Schema generation-and-validation pass that runs before encoding
  a request. Structural errors (unknown child, wrong shape, bad
  instance-identifier) still surface from the encoder itself;
  range/pattern/length constraint validation does not. `-c`/
  `--continue`'s error-vs-warning distinction is correspondingly
  shallow (logged, not threaded through to change encoding behavior on
  partial failure).
- **Topology-file device default (`.mscc-libeasy-topology.yaml`)**: not
  verified at all -- no unit test, and every hardware run so far has
  passed `-d` explicitly rather than relying on this lookup.
