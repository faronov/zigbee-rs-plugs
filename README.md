# zigbee-rs-plugs

Pure-Rust, heap-free smart-plug products built on
[`zigbee-rs@14ba6df`](https://github.com/faronov/zigbee-rs/tree/14ba6df7309602cc31a08e97667c732d61b9580d).

This development branch is the plug-side proof of the cross-platform
application model. It keeps application/profile behavior, product policy,
physical board resources, and chip HAL mechanisms separate:

```text
plug profile + shared plug behavior
        |
plug-router-app
        |
router-app + zigbee-runtime
        |
product policy/storage + board resources + platform MAC/HAL
```

## Cross-platform application model

[`plug-router-app`](apps/plug-router/) contains the shared smart-plug lifecycle:
local relay reconciliation, protection vetoes, bounded metering service,
meter-health fail-closed latching, 100 ms On/Off processing, wear-bounded
application checkpoints, network-status mapping, and synchronous relay-off
factory-reset ordering.

- `PlugRouterApp` composes that behavior with
  `router_app::ParentRouterApp`. It requires a parent-capable MAC and a durable
  child-table store. All six TLSR8258 products use this path.
- `AlwaysOnEndDevicePlugApp` composes the same private behavior core with
  `router_app::AlwaysOnEndDeviceApp`. The EFR32 proof uses this path because
  `Efr32s2Mac` does not implement `ParentMacDriver`; it is a conformant
  receiver-on End Device, compiles without `zigbee-runtime/router`, and makes
  no routing or child-service claim.

Firmware `main.rs` files are composition roots only. Board crates own fitted
pins and exclusive peripheral/storage tokens. Product crates own identity,
flash partitioning, persistence construction, profile/policy selection, and
deployment constraints.

See [docs/architecture.md](docs/architecture.md) for the complete ownership,
persistence, and reset model.

## Validation status

| Target | Role | Validation reached | Not proven |
|---|---|---|---|
| Six TLSR8258 plug products | Child-capable parent `Router` | Host tests plus tc32 compile, link, layout, symbol, AES-backend, and size gates | Any physical plug operation |
| EFR32MG21 BRD4181A proof | Always-on `EndDevice`, no routing/children | Host tests plus locked ARM release build and ELF/source/layout verifier | GPIO, flash, radio, commissioning, reset, safety, metering, or long-duration behavior on hardware |

The EFR32 target is a **relay-only portability proof**. Its PC3/EXP10 signal is
only a low-voltage logic output, and its meter supplies labeled synthetic
zero-load data. It is not a mains-switching or metering implementation.

CI artifacts are experimental build evidence, not release images. No image in
this repository is hardware-qualified, and no OTA image is published.

## Product targets

The TLSR8258 firmware has exactly six compile-time product features:

| Feature | Stock identity | Board/meter | Flash |
|---|---|---|---:|
| `tz3000-gjnozsaz-1m` | `_TZ3000_gjnozsaz` / `TS011F` | TS011F PC2 relay, BL0942 UART | 1 MiB |
| `tz3000-gjnozsaz-512k` | `_TZ3000_gjnozsaz` / `TS011F` | TS011F PC2 relay, BL0942 UART | 512 KiB |
| `tz3000-w0qqde0g` | `_TZ3000_w0qqde0g` / `TS011F` | TS011F PC2 relay, BL0942 UART | 1 MiB |
| `tz3000-zloso4jk` | `_TZ3000_zloso4jk` / `TS011F` | TS011F PC2 relay, BL0942 UART | 1 MiB |
| `legacy-bl0937-pd6` | unknown manufacturer / `TS011F` | legacy PD6 relay, BL0937 pulses | 1 MiB |
| `zbeacon-ts011f-512k` | `Zbeacon` / `TS011F` | Zbeacon PD2 relay, BL0937 pulses | 512 KiB |

`_TZ3000_gjnozsaz` exists with two flash geometries. A model or manufacturer
string alone is not sufficient to select a safe image. Exact build commands,
current measured sizes, partition constraints, and hardware gates are in
[`firmware/tlsr8258-plug/README.md`](firmware/tlsr8258-plug/README.md).

## Repository layout

```text
apps/plug-router/       shared cross-platform router plug application
drivers/                metering-chip protocols and conversion
plug-core/              hardware-independent settings/protection/records
plug-controller/        shared relay, button, LED, and protection behavior
zigbee-plug-profile/    reusable Smart Plug endpoint and ZCL mapping
boards/                 fitted pins and exclusive physical resources
products/               identity, policy, storage, and memory layout
plug-storage/           TLSR8258 child/app/security partition mechanisms
firmware/               platform composition roots
link/                   canonical TLSR8258 linker scripts
scripts/                build and post-link validation helpers
docs/                   architecture, hardware, evidence, and safety
```

## Development checkout

The manifests pin every core/HAL dependency to immutable `zigbee-rs` commit
[`14ba6df7309602cc31a08e97667c732d61b9580d`](https://github.com/faronov/zigbee-rs/commit/14ba6df7309602cc31a08e97667c732d61b9580d).
Cargo fetches that revision directly; an adjacent core checkout is not
required.

The workspace and EFR proof use Rust `1.98.0` from `rust-toolchain.toml`:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo build --workspace --release --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- \
  -D warnings -A clippy::chunks-exact-to-as-chunks
cargo doc --workspace --no-deps --locked
```

The two firmware roots are validated separately:

- TLSR8258: use `scripts/tlsr8258-firmware.sh`; see the
  [firmware README](firmware/tlsr8258-plug/README.md).
- EFR32 proof:

  ```bash
  cd firmware/plug-efr32-proof
  cargo build --release --locked
  python3 tools/verify-proof.py \
    target/thumbv8m.main-none-eabihf/release/plug-efr32-proof
  arm-none-eabi-size \
    target/thumbv8m.main-none-eabihf/release/plug-efr32-proof
  ```

Development links in these documents intentionally target the reviewed
[`14ba6df`](https://github.com/faronov/zigbee-rs/tree/14ba6df7309602cc31a08e97667c732d61b9580d)
core revision.
The public zigbee-rs GitHub Pages book is deployed from the core repository's
main/deploy path, so it will not describe this migration until the core branch
is merged and Pages is deployed.

Read [docs/safety.md](docs/safety.md) before connecting any programmer or
load. Hardware and protocol provenance is listed in
[docs/sources.md](docs/sources.md).
