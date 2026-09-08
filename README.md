# rcc

`rcc` is a command-line tool for talking to Microchip VelocityDRIVE-SP
network switch devices. It connects over a serial link (or a
network-bridged serial connection, e.g. via a lab terminal server) and
speaks the device's management protocol stack -- MUP1 framing, CoAP, and
CORECONF/YANG-modeled configuration data -- so you can read and write a
device's configuration and state from the command line or from scripts.

The first time you connect to a device, `rcc` downloads the YANG data
model matching its firmware automatically, so it always knows how to
interpret that device's configuration and state correctly, even across
firmware versions with different capabilities.

## Building

Requires a Rust toolchain ([rustup.rs](https://rustup.rs) is the easiest
way to get one). No other system packages are needed to build.

```
cargo build --release
```

The binary is then at `target/release/rcc`. Alternatively,
`cargo install --path rcc` builds and installs it straight to
`~/.cargo/bin` (make sure that's on your `PATH`)

`curl` needs to be installed for the automatic YANG data model download
mentioned above; everything else has no runtime dependencies beyond the
operating system itself.

## Usage

```
rcc -d /dev/ttyACM0 -m get
echo '- "/ietf-system:system-state/platform/os-version"' | rcc -d /dev/ttyACM0 -m fetch
```

`-d` is the serial device (or `termhub://host:port` / `telnet://host:port`
for a network-bridged connection); `-m` is the CoAP method
(`get`/`fetch`/`ipatch`/`put`/`post`/`delete`). Run `rcc --help` for the
full flag reference.

## Relationship to `mup1cc`

`rcc` is a from-scratch reimplementation of Microchip's `mup1cc`
([source](https://github.com/microchip-ung/velocitydrivesp-support/blob/main/support/scripts/mup1cc)),
a Ruby tool that does the same job. If you've used `mup1cc` before, `rcc`
accepts the same command-line flags and behaves the same way.
You don't need to know anything about `mup1cc` to use `rcc`;
where the two differ, it's noted below.

### What's different from `mup1cc`

- **A single, self-contained program.** No Ruby interpreter or gems to
  install -- `rcc` is one binary.
- **Friendlier validation errors.** Requests are checked against the
  device's YANG data model before being sent, with messages that say
  what's actually wrong (e.g. "must be between 0 and 65535") rather than
  a raw type mismatch.
- **No networking or TLS code built in.** Downloading a device's YANG
  data model is done by running `curl` as a separate program, not by an
  HTTP library bundled into `rcc`. You can point `--catalog-fetcher` at
  your own program or script instead, to fully control how that download
  happens (a different mirror, a proxy, custom certificates, whatever
  your environment needs) -- see `rcc --help`.
- **Runs on Windows as well as Linux and macOS.**

### Not (yet) supported, compared to `mup1cc`

- **DTLS** (`-k`): not implemented. Passing a key file produces a clear
  error.
- **`--log-append`/`--log-msg`/`--log-run`/`--log-steps`**: not
  implemented / deprecated.
- **`-s`/`--sys-trace`**: accepted for compatibility (so a script that
  passes it won't break) but has no effect.
