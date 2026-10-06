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
[`14ba6df7309602cc31a08e97667c732d61b9580d`](https://github.com/faronov/zigbee-rs/tree/14ba6df7309602cc31a08e97667c732d61b9580d);
no adjacent checkout is required.
The public core GitHub Pages book will not include this model until the branch
is merged and Pages is deployed.

Firmware builds require the modern-tc32
[`tc32-1.98.1-20261003-31a272`](https://github.com/modern-tc32/rust/releases/tag/tc32-1.98.1-20261003-31a272)
toolchain, the same release the pinned zigbee-rs core's tc32 CI uses. It
reports `rustc 1.98.1-dev` and LLVM 23.1.2. CI caches the release archive and
re-verifies its SHA-256
`d72e68cfb7490583911323c358fd9dcc760baaa7ebdf1145cdc7208bae6b931b` on every
run, including cache hits. `.cargo/config.toml` enables LLVM tail merging
(`-enable-tail-merge=true`), matching the core's TLSR8258 router example.

Install it at the default path:

```text
.toolchains/tc32-1.98.1-20261003-31a272/
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

The following local measurements were regenerated on 2026-10-06 from commit
`b02b7f5` (zigbee-rs core `14ba6df`) with `tc32-1.98.1-20261003-31a272` and
LLVM tail merging. Every entry passed the layout check and symbol gate. They
are build evidence, not stable release identifiers or hardware proof.

| Product | Image | Headroom to `0x70000` | RAM code | SHA-256 |
|---|---:|---:|---:|---|
| `tz3000-gjnozsaz-1m` | 307,492 B | 151,260 B | 3,620 B | `80118f9f31f894940be043e023ebb9c348e95bd21b408b3e623c8bacc951aed3` |
| `tz3000-gjnozsaz-512k` | 307,496 B | 151,256 B | 3,624 B | `c9e64d83053be6c58255eeb3478ebdf065097e5179c82abf6f03236d0fc8c541` |
| `tz3000-w0qqde0g` | 307,492 B | 151,260 B | 3,620 B | `aa434c7fd7b5e8d40532240efa28ce75fc96b4343b64e2c9be14ac18c3ab10a8` |
| `tz3000-zloso4jk` | 307,492 B | 151,260 B | 3,620 B | `e93d512810a868f1ccd0808e39e617a7901dfc7f8e00b77660c72d315c25510c` |
| `legacy-bl0937-pd6` | 308,864 B | 149,888 B | 3,804 B | `e8a117d5161582eceeb0c32d54c4135a4a539df178340621be3acd96524c9760` |
| `zbeacon-ts011f-512k` | 309,040 B | 149,712 B | 3,808 B | `7134861ac1222296d0516ce03ff9083a31a6835a8a5396acfe5f66ff775092a8` |

For comparison, the previous core pin `1d7df8f` built with the retired
`tc32-stage2-tc32-45` toolchain produced 371,400 B for `tz3000-gjnozsaz-1m`;
the current core no longer fits below `0x70000` with that toolchain.

Use the generated `*.size.json` from a particular build as the source of
truth for that artifact.

## Button behavior

| Gesture (30 ms debounce) | Plug state | Action |
|---|---|---|
| short press | protection or meter trip latched | clear the trip; power returns only after a fresh safe meter sample |
| short press | joined | toggle the relay; the change is copied into ZCL OnOff and reported |
| short press | not joined | request Network Steering; the LED starts blinking |
| hold for 4 s | any | relay Off, factory reset, then immediate Network Steering |

Network state at power-up and after a Leave follows zigbee-rs `14ba6df`:

- a never-commissioned plug starts steering by itself on power-up;
- a commissioned plug resumes its network and does not steer;
- after a coordinator Leave/Remove or a network-requested reset, the plug
  becomes factory-new, keeps the LED dark, and does not search until a
  short press. This keeps a deliberately removed plug off the air.

The LED blinks while searching or joining, follows the relay once joined, and
is solid on a fault.

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
