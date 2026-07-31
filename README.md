# zigbee-rs-plugs

Pure-Rust smart-plug components built on
[`zigbee-rs`](https://github.com/faronov/zigbee-rs).

The repository keeps metering drivers, physical boards, flash/product policy,
and Zigbee application behavior separate. It does not reuse the implementation
from older experimental firmware; only independently documented hardware
connections and public chip protocols are used.

## Current status

`v0.1.0-alpha.1` provides:

- original `no_std` BL0937 pulse decoding and energy accumulation;
- original `no_std` BL0942 UART framing, checksum, decoding, and commands;
- relay protection, restart, settings, and durable energy-record logic,
  composed into a host-testable `zigbee-plug-controller` (relay/LED
  reconciliation, button debounce/toggle, protection trip/latch, startup
  policy);
- typed TLSR8258 board resources for the two known incompatible pin maps;
- separate product profiles for each known Tuya fingerprint/flash geometry;
- type-safe, single-split TLSR8258 application-NV and Zigbee security-journal
  flash partitions (`zigbee-plug-storage`), and the canonical 512 KiB/1 MiB
  linker scripts and build/check helper that enforce those boundaries;
- a `zigbee-rs` Smart Plug profile adapter and host-side example;
- a `no_std`/`no_main` `firmware/tlsr8258-plug` crate: one binary, exactly
  one of five compile-time product features, a production router loop
  adapted from `zigbee-rs`'s own `examples/telink-tlsr8258-router`, and the
  BL0942 UART / BL0937 capture metering tasks;
- a TC32 CI workflow (`.github/workflows/build-tc32.yml`) that matrix-builds
  all five product features and layout-checks each resulting image;
- host tests, Clippy, formatting, and documentation CI.

GitHub Actions builds and uploads five explicitly experimental `.bin`
artifacts, but no release or OTA image is published and none has run on
TLSR8258 hardware. The firmware is pinned to the published `zigbee-rs`
TLSR8258 HAL revision containing UART, capture, geometry-aware identity, and
the ADC-backed flash-voltage guard; the remaining gates are physical-board
validation, not missing upstream APIs.

## Hardware profiles

| Product target | Meter | Flash | Evidence |
|---|---|---:|---|
| `tz3000-w0qqde0g` | BL0942, PB1/PB7 UART | 1 MiB | documented board family |
| `tz3000-zloso4jk` | BL0942, PB1/PB7 UART | 1 MiB | documented board family |
| `tz3000-gjnozsaz-1m` | BL0942, PB1/PB7 UART | 1 MiB | documented board family |
| `tz3000-gjnozsaz-512k` | BL0942, PB1/PB7 UART | 512 KiB | experimental small-flash variant |
| `legacy-bl0937-pd6` | BL0937, PB5/PB6/PB7 | assumed 1 MiB | old pin map; Tuya fingerprint unknown |

`_TZ3000_gjnozsaz` exists with at least two flash geometries under the same
manufacturer name. A model string alone is therefore not sufficient to select
a safe image.

## Layout

```text
drivers/       chip protocol and measurement conversion
plug-core/     relay, protection, settings, and persistence logic
plug-controller/  host-testable controller composing plug-core + zigbee-plug-profile
plug-hardware/ board/product metadata and flash validation
plug-storage/  TLSR8258 application-NV/security-journal flash partitioning
boards/        TLSR8258 ownership of exact physical pins
products/      one fail-closed build profile per model and flash geometry
zigbee-plug-profile/  Zigbee Electrical Measurement/Metering mapping
firmware/      no_std/no_main TLSR8258 smart-plug binary (one product feature)
examples/      host-side behavior example
link/          canonical 512 KiB/1 MiB TLSR8258 linker scripts
scripts/       TLSR8258 build/check helper (see link/README.md)
```

## Development

Rust 1.88 or newer is required.

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --no-deps
```

`firmware/tlsr8258-plug` is excluded from the root workspace (it is
`no_std`/`no_main` and targets `tc32-unknown-none-elf` only), so the
commands above never try to cross-compile it. See
[`firmware/tlsr8258-plug/README.md`](firmware/tlsr8258-plug/README.md) for
its own build/check commands and current hardware-gate status.

`scripts/tlsr8258-firmware.sh` builds and layout-checks a TLSR8258 firmware
binary against the canonical linker scripts in `link/` using the modern-tc32
toolchain, given an explicit crate directory, binary name, linker layout, and
product feature. It never flashes a device — see
[docs/safety.md](docs/safety.md). `.github/workflows/build-tc32.yml` runs it
for all five product features on every push/PR touching the firmware crate
or its dependencies.

Read [docs/safety.md](docs/safety.md) before opening or flashing any mains
device. Hardware and protocol provenance is listed in
[docs/sources.md](docs/sources.md).
