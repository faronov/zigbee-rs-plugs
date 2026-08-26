# `tlsr8258-plug` firmware

`no_std`/`no_main` TLSR8258 smart-plug firmware with exactly one compile-time
product feature.

**Experimental:** every product compiles, links, and passes the repository's
post-link gates. No image from this crate has run on physical TLSR8258 plug
hardware. The build helper has no flashing command.

## Shared application composition

The firmware modules are composition roots, not separate applications:

```text
selected product identity/layout/storage
  + selected board pins/resources
  + TelinkMac<Router> and PersistentChildren
  + BL0942 or BL0937 MeterService
  + TlsrLocalControl and TlsrClock
  + plug_router_app::PlugRouterApp
```

`PlugRouterApp` owns the common relay/protection, local-control, metering,
checkpoint, reset, and finite network-step lifecycle. The BL0942 and BL0937
files only construct their fitted resources. Both use
`router_app::ParentRouterApp`, the typed `zigbee_runtime::role::Router`, and a
durable child-table journal.

The private shared core is also used by the EFR32 proof through
`AlwaysOnEndDevicePlugApp`; that proof selects a conformant receiver-on
`EndDevice` with the routing feature compiled out, while all six TLSR8258
products explicitly enable parent capability.

## Required core revision and toolchain

The manifests fetch the core crates from immutable `zigbee-rs` revision
[`002d47923ab910afd46b771cc1b4d2eaee1696c9`](https://github.com/faronov/zigbee-rs/tree/002d47923ab910afd46b771cc1b4d2eaee1696c9);
no adjacent checkout is required.
The public core GitHub Pages book will not include this model until the branch
is merged and Pages is deployed.

Firmware builds require the modern-tc32
`tc32-stage2-tc32-45` toolchain. The checked local toolchain reports
`rustc 1.97.0-dev`, `cargo 1.97.0-dev`, and LLVM 23. CI downloads the archive
whose SHA-256 is
`916732a6f5e19da722e735cb267192dbd5dda155f0701287fab52a54b852c9b7`.

Install it at the default path:

```text
.toolchains/tc32-stage2-tc32-45/
```

or set `TC32_TOOLCHAIN` to another installation root. The helper uses that
toolchain's `cargo`, `llvm-nm`, and `llvm-objcopy`.

## Six product features

Exactly one feature is required. Zero or multiple features fail at compile
time.

| Feature | Product crate identity | Board/meter | Layout | Factory/read-only | Stock OTA catalog |
|---|---|---|---:|---|---|
| `tz3000-gjnozsaz-1m` | `_TZ3000_gjnozsaz` / `TS011F`, documented | TS011F PC2 / BL0942 | 1 MiB | `0xFE000..0x100000` | `0x1141` / `0xD3A3` |
| `tz3000-gjnozsaz-512k` | `_TZ3000_gjnozsaz` / `TS011F`, experimental | TS011F PC2 / BL0942 | 512 KiB | `0x76000..0x78000` | `0x1286` / `0x0002` |
| `tz3000-w0qqde0g` | `_TZ3000_w0qqde0g` / `TS011F`, documented | TS011F PC2 / BL0942 | 1 MiB | `0xFE000..0x100000` | `0x1141` / `0xD3A3` |
| `tz3000-zloso4jk` | `_TZ3000_zloso4jk` / `TS011F`, documented | TS011F PC2 / BL0942 | 1 MiB | `0xFE000..0x100000` | `0x1141` / `0xD3A3` |
| `legacy-bl0937-pd6` | unknown manufacturer / `TS011F`, pin-map only | legacy PD6 / BL0937 | 1 MiB | `0xFE000..0x100000` | none |
| `zbeacon-ts011f-512k` | `Zbeacon` / `TS011F`, pin-map only | Zbeacon PD2 / BL0937 | 512 KiB | `0x76000..0x78000` | none |

The two `_TZ3000_gjnozsaz` products share a manufacturer/model identity but
not a flash geometry. Never select between them from the string alone.
The OTA values are stock catalog metadata only; this firmware does not
advertise or consume them, and OTA remains disabled.

## Exact build commands

Run from the repository root:

```bash
scripts/tlsr8258-firmware.sh build \
  firmware/tlsr8258-plug tlsr8258-plug 1m \
  tz3000-gjnozsaz-1m

scripts/tlsr8258-firmware.sh build \
  firmware/tlsr8258-plug tlsr8258-plug 512k \
  tz3000-gjnozsaz-512k

scripts/tlsr8258-firmware.sh build \
  firmware/tlsr8258-plug tlsr8258-plug 1m \
  tz3000-w0qqde0g

scripts/tlsr8258-firmware.sh build \
  firmware/tlsr8258-plug tlsr8258-plug 1m \
  tz3000-zloso4jk

scripts/tlsr8258-firmware.sh build \
  firmware/tlsr8258-plug tlsr8258-plug 1m \
  legacy-bl0937-pd6

scripts/tlsr8258-firmware.sh build \
  firmware/tlsr8258-plug tlsr8258-plug 512k \
  zbeacon-ts011f-512k
```

For compile-only validation, replace `build` with `check`; the other four
arguments must remain explicit and geometry-correct.

`build` performs a locked release `cargo rustc`, converts the ELF to `.bin`,
checks the linker/RAM/cache/RF-DMA layout, checks linked role/child/AES
symbols, and writes `tlsr8258-plug.size.json`. It never flashes hardware.

## Flash partitions and constraints

All six products reserve:

| Range | Purpose |
|---|---|
| `0x00000..0x70000` | firmware image budget |
| `0x70000..0x72000` | durable child-table journal |
| `0x72000..0x74000` | application-state log |
| `0x74000..0x76000` | credentials and crash-safe frame-counter bounds |

The binary must end **strictly before** `0x70000` (458,752 bytes). The linker
and build helper both enforce that boundary.

On 1 MiB parts, `0x96000..0xFC000` is a documented but disabled candidate
region. No product storage accessor exists for it. Unlisted gaps and the
geometry-specific factory regions are not spare application flash.

Each board creates one `OnboardFlash` token. The selected product consumes it
once and constructs three disjoint stores. Product selection therefore owns
the partition contract; the board owns only the fitted flash resource.

## Current measured builds

The following local measurements were regenerated from the current dirty
worktree on 2026-08-26 with `tc32-stage2-tc32-45`. They are build evidence,
not stable release identifiers or hardware proof.

| Product | Image | Headroom to `0x70000` | RAM code | SHA-256 |
|---|---:|---:|---:|---|
| `tz3000-gjnozsaz-1m` | 371,400 B | 87,352 B | 4,836 B | `20cbfc70a96b1689824a4fcea9571b1b56c6b312b3efa7090d1cb8ebcec6e75a` |
| `tz3000-gjnozsaz-512k` | 371,396 B | 87,356 B | 4,840 B | `750866b6dd57c1a200a6ccd02e23aa1391aa59eb1f8d3be311270c7c22a4260e` |
| `tz3000-w0qqde0g` | 371,400 B | 87,352 B | 4,836 B | `61d7c0376235cd5ad7ed17a3535e5a2c7da5a8ac2ca339c3a927a39384b135cc` |
| `tz3000-zloso4jk` | 371,400 B | 87,352 B | 4,836 B | `ce909d75a11368fe353038b0f8abab3b761e8c9ee311abea088c12f98e630cd9` |
| `legacy-bl0937-pd6` | 377,864 B | 80,888 B | 5,120 B | `db6acb53a16405747fc9e0155d218e63617ffffd203e99ef95f2ef5a4cb1945e` |
| `zbeacon-ts011f-512k` | 377,980 B | 80,772 B | 5,124 B | `520091b146eaba8a72ad4e1a3767786412086ec48b1c96947502095d4ce77e1a` |

Use the generated `*.size.json` from a particular build as the source of
truth for that artifact.

## Reset and persistence behavior

The shared app handles a four-second local reset before any already-due retry:

1. force logical relay Off and synchronously acknowledge physical relay Off;
2. checkpoint relay Off and accumulated energy in application NV;
3. write credential-free security state while preserving global/TCLK counter
   upper bounds;
4. write an empty durable child table;
5. schedule immediate recommissioning;
6. begin steering on the next application step.

An application checkpoint error stops before security reset, child clear, or
steering. Any later persistence/network error enters the firmware fault path:
relay Off, fault LED On, and no further application progress.

The relay is also held physically Off until the first safe meter sample.
Capture overflow or UART reset latches a meter fault immediately; missing or
stale samples latch after the shared 10-second health deadline. Timer1 owns a
sticky physical inhibit for those deadlines and the four-second reset, so
network scanning/rejoin cannot defer relay-off or clear it through ordinary
reconciliation. A local fault clear starts a fresh sample interlock rather
than restoring power directly.

Do not raw-erase `0x74000..0x76000` during factory reset. After a stock-to-Rust
migration, remove the coordinator's old entry before first Rust commissioning
if it retains the same factory EUI-64 and a higher replay floor. Once Rust has
commissioned, preserve the security journal so counter bounds are never
reused.

The child journal is separate because its larger, lower-frequency snapshots
must not enlarge every security-counter reservation. Records are bound to the
network extended PAN ID; corrupt or foreign child state is replaced with an
empty durable snapshot before parent service.

## Hardware and security gates

The linked-image gate proves that every product image contains:

- TLSR8258 `HardwareAes128` and `install_aes_engine`;
- typed `zigbee_runtime::role::Router`;
- the parent/child-serving path and `ChildTableJournal`.

It proves that the image omits:

- RustCrypto software AES;
- `RamChildTableStore`;
- `EndDevice`, forwarding-only `RelayRouter`, and End Device Timeout client
  code.

The board resources also reserve PC5 plus ADC for the geometry-aware Zbit
flash-voltage guard. Startup fails closed if the declared capacity, JEDEC
geometry, ADC calibration, hardware AES installation, storage construction,
or factory identity path fails.

These are still open hardware gates for every product:

1. preserve and inspect the exact stock flash;
2. verify live JEDEC geometry and PC5's electrical connection;
3. prove inactive-at-boot relay behavior, LED, button, four-second reset, and
   local control during scan/retry/rejoin;
4. prove BL0942 UART or BL0937 capture/calibration against a known load;
5. capture commissioning, parent/child service, ZDO interview, commands, and
   reporting;
6. power-cycle through application, child, and security journal writes and
   rollover without counter reuse;
7. test fault/reset/brownout behavior without a mains load first;
8. keep OTA disabled until each product has verified image identity, staging,
   verification, activation, rollback, and persistence-retention policy.

`.github/workflows/build-tc32.yml` runs the six-command matrix and uploads
experimental ELF, BIN, and size JSON artifacts for 30 days. It has no flash
job.
