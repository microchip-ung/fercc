# rcc

A Rust CLI clone of `mup1cc`: a MUP1/CoAP/CORECONF client for
VelocityDRIVE-SP devices.

`mup1cc` ([also on GitHub](https://github.com/microchip-ung/velocitydrivesp-support/blob/main/support/scripts/mup1cc))
is the reference implementation. Any behavioral discrepancy between this
port and that tool is most likely a bug in this port, not in the
reference.

Status as of this writing: **core CLI parity implemented and verified
against real hardware** (`/dev/ttyACM0`), including a byte-for-byte
comparison of `rcc -m get -w` output against the real Ruby tool run
against the same board (see git log for the diff session), and **the
codec conformance fixture corpus passes byte-exact** (`cargo test`).
DTLS, `--log-*`, and a handful of smaller gaps are intentionally out of
scope for this pass -- see "Known gaps" below.

## Building

```
cargo build --workspace            # debug
cargo build --workspace --release  # optimized
cargo test --workspace             # unit tests + the codec conformance
                                    # fixture suite (yang/tests/fixtures.rs)
```

Requires a stable Rust toolchain (developed against 1.98.1; no nightly
features used). No system packages needed -- `serialport` is built with
`default-features = false` (no libudev) since the device path is always
given explicitly.

## Running

```
cargo run -p rcc -- -d /dev/ttyACM0 -m get -w
echo '- "/ietf-system:system/contact"' | cargo run -p rcc -- -d /dev/ttyACM0 -m fetch -w
```

Flags mirror `mup1cc`'s `OptionParser` block -- see `rcc/src/opts.rs` or
`rcc --help`.

`rcc conv`/`rcc schema` fold in `support/yang-enc/yang-enc.rb`'s CLI
(format conversion and JSON-Schema generation) as subcommands on the same
binary, matching that standalone tool's offline nature exactly: no `-d`,
no checksum-download fallback, no `-w`/`--no-workspace` at all -- just
the workspace catalog by default, or an explicit `.yang`/`.sid` set given
on the command line:

```
echo '- "/ietf-system:system/contact"' | cargo run -p rcc -- conv -i yaml -o cbor -c fetch
cargo run -p rcc -- schema > schema.json
```

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
  - `json_schema.rs` -- draft-07 JSON Schema generation from a built
    schema, porting `yang-enc.rb`'s `to_json_schema`/`type2schema`
    (`rcc schema`'s output, and the schema `rcc conv`'s eventual
    validation gate will check requests against).
  - `catalog.rs` -- workspace (`support/scripts/gen-cc-nodes.yaml`) and
    checksum-download catalog acquisition, porting
    `support/yang-enc/yang-schema.rb` minus its on-disk parsed-schema
    cache (re-parsing fresh every invocation is the explicit point of
    doing this in Rust). There's no HTTP/TLS library anywhere in this
    codebase: the download itself is always an external command --
    `curl` by default (matching the real Ruby reference's own `wget`
    backtick call, `support/scripts/mup1cc:98-103`), or whatever
    `--catalog-fetcher <command>` names instead. The contract: called
    as `<command> <checksum>`, write the catalog's `.tar.gz` bytes to
    stdout and exit 0 -- `rcc` extracts them itself, so the command
    doesn't need to leave any files behind, and it's entirely up to
    that command how network security (TLS, proxies, certificates,
    alternate mirrors) is handled.
- `rcc/` -- the CLI binary tying it all together.
- `test-data/` -- a curated set of YAML/CBOR fixture pairs plus a bundled
  YANG catalog tarball. Originally ~387 pairs copied wholesale from
  `~/sw-velocitydrive-devclient`'s `test-data/` (a prior TypeScript port
  of this same stack); trimmed by measured code coverage (line, branch,
  function, region) to the 12 that already cover everything the full set
  did, plus 4 real hardware-captured negative cases -- see
  `yang/tests/fixtures.rs`'s doc comments for the method. This is the
  primary codec conformance corpus, alongside `support/yang-enc/spec/tests/`
  in the main Ruby codebase.

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
- **Validation**: implemented natively rather than via a generated JSON
  Schema (`yang::json_schema::to_json_schema` still exists independently,
  for `rcc schema`) -- range/length/pattern/enum/bit/identity/required
  checks live directly in `yang/src/codec.rs`'s encoder
  (`type_to_cbor`/`encode_body`), right next to the type-shape checks
  they're a natural extension of. `-c`/`--continue` has real, per-field
  leniency matching the Ruby reference's `json2cbor_hash`/`type2cbor`
  exactly: without it, any violation (an unknown field, a missing
  mandatory field or list key, an out-of-range/wrong-length/pattern-
  mismatched value, a malformed leaf) hard-fails with a message stating
  the actual constraint (e.g. "must be between 0 and 65535"), not just a
  type name. With it, the request still gets sent: the offending field
  is skipped (container/list-shape or unknown-field errors) or passed
  through as its raw JSON value (a leaf that fails to encode), matching
  what Ruby's own `--continue` does. See `yang/src/codec.rs`'s test
  module for the full battery of these cases, both ways. `anydata
  board:factory_default_config` is schematized as `{}` (any value) by
  `to_json_schema` specifically, rather than the Ruby reference's
  recursive re-derivation of the *default* schema under content-format
  `put` -- a narrow gap in that one generated-schema feature, unrelated
  to request validation.
- **Input/output format selection (`-i`/`-o`/`-I`/`-O`)**: the
  flag/extension/default priority chain is unit-tested for both JSON
  and YAML, in both directions. Reading from STDIN specifically (as
  opposed to `-i FILE`) is not covered by any test; JSON output has
  been spot-checked against real hardware, JSON input has not.
- **Topology-file device default (`.mscc-libeasy-topology.yaml`)**: not
  verified at all -- no unit test, and every hardware run so far has
  passed `-d` explicitly rather than relying on this lookup.
