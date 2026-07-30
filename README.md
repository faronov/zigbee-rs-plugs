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
- relay protection, restart, settings, and durable energy-record logic;
- typed TLSR8258 board resources for the two known incompatible pin maps;
- separate product profiles for each known Tuya fingerprint/flash geometry;
- a `zigbee-rs` Smart Plug profile adapter and host-side example;
- host tests, Clippy, formatting, and documentation CI.

No flashable plug firmware or OTA image is published yet. The current
`zigbee-rs` TLSR8258 HAL still needs a hardware-proven UART path for BL0942 and
GPIO edge capture for BL0937. Publishing a binary before those gates pass
would incorrectly imply hardware support.

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
plug-hardware/ board/product metadata and flash validation
boards/        TLSR8258 ownership of exact physical pins
products/      one fail-closed build profile per model and flash geometry
zigbee-plug-profile/  Zigbee Electrical Measurement/Metering mapping
examples/      host-side behavior example
```

## Development

Rust 1.88 or newer is required.

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --no-deps
```

Read [docs/safety.md](docs/safety.md) before opening or flashing any mains
device. Hardware and protocol provenance is listed in
[docs/sources.md](docs/sources.md).
