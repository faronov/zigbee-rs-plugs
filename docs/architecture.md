# Architecture

The repository follows the same ownership split as `zigbee-rs`:

1. Metering drivers accept bytes or pulse counts and have no board knowledge.
2. Board crates own physical GPIOs and encode active levels.
3. Product crates select one board, flash geometry, and stock identity.
4. `zigbee-plug-core` owns safety and persistence *policy* (protection
   engine and CRC-protected energy/relay record formats) independent of
   any chip.
5. `zigbee-plug-storage` owns TLSR8258 flash-partition *mechanism*: it turns
   a board's single `OnboardFlash` token into the product's application-NV
   and Zigbee security-journal accessors (see below).
6. `zigbee-plug-profile` maps validated measurements into standard Zigbee
   clusters.
7. `zigbee-plug-controller` owns the shared, host-testable device-behavior
   composition (relay desired/actual reconciliation, button debounce,
   LED policy, protection-engine wiring, command/trip/latch semantics)
   used by every product's firmware — no per-product or per-chip code.
8. `firmware/tlsr8258-plug` is the single no_std/no_main firmware crate that
   composes exactly one product target at compile time (one of six
   mutually exclusive Cargo features) into a production BDB/ZCL router
   loop, adapted from `zigbee-rs`'s
   `examples/telink-tlsr8258-router`. It is excluded from this workspace
   (see "Firmware crate" below) but still built from shared, reusable
   modules (metering tasks, persistence, router support) with no
   per-product branching beyond the feature-selected board/product crate.

Product selection is intentionally compile-time. Runtime guessing between
different relay pins, metering ICs, or flash layouts can energize a relay or
erase factory data on the wrong PCB.

## Controller crate (`plug-controller`)

`zigbee_plug_controller::PlugController` is the one, chip-independent place
that composes:

- `zigbee_plug_profile::ZigbeePlug` plus
  `zigbee_plug_core::ProtectionEngine` (relay desired/actual state and
  protection latch),
- button debounce, short-press relay toggle, and one-shot four-second local
  network factory-reset gesture,
- network-status LED policy (dark while offline, one-hertz blink while
  commissioning/rejoining, solid while joined or faulted),
- `ZigbeePlug`'s ZCL On/Off mandatory 100 ms timers, and
- electrical-sample ingestion that feeds the protection engine.

Its invariant, enforced by host tests: a remote or local On command can never
energize the relay while protection is tripped. Non-voltage trips require an
explicit latch-clear; voltage auto-restart occurs only when a product
explicitly enables it, and is off by default. Every router loop
(`bl0942_app.rs`, `bl0937_app.rs`) calls the same `PlugController` API after
every incoming Zigbee frame and every controller tick; no router loop
reimplements relay/trip logic itself.

## Firmware crate (`firmware/tlsr8258-plug`)

`firmware/tlsr8258-plug` is a `no_std`/`no_main` binary crate excluded from
this Cargo workspace (it targets `tc32`, which the host toolchain cannot
build or lint). It selects exactly one of six mutually exclusive Cargo
features — one per product in `products/` — and fails to compile if zero or
more than one is selected (`compile_error!` in `src/main.rs`). The four
BL0942 products share one router loop (`bl0942_app.rs`) and metering task
(`bl0942_task.rs`); both BL0937 products share a separate loop
(`bl0937_app.rs`) and capture-based metering task (`bl0937_task.rs`), while
retaining distinct board crates, since their measurement chip has no UART
and instead requires two edge-capture
channels. Product model/manufacturer identity and flash layout always come
from the selected product crate — the firmware never fingerprints hardware
at runtime to infer which product it is.

Both router loops start Timer0 before constructing the MAC, ADC guard, or
persistence stack. Their controller/metering/checkpoint timestamps use
`TickMillis` to extend Timer0's roughly 179-second raw counter wrap into a
normal wrapping `u32` millisecond clock; directly dividing the raw counter
would reset application time on every hardware wrap.

`local_control.rs` owns the relay, status LED, and button GPIO tokens after
startup and services them from a dedicated 10 ms Timer1 interrupt. This keeps
short-press relay control independent of BDB/MAC progress even while the
single-threaded Telink operations are inside bounded synchronous waits. The
interrupt only updates physical state and small flags; the main loop later
reconciles a local relay selection into the ZCL OnOff attribute. Protection
remains authoritative, and a four-second hold requests network factory reset
without first toggling the relay.

`persistence.rs` is shared by both loops: it restores the last checkpointed
relay/energy state from the product's `ApplicationNv` before BDB profile
startup, and writes a new CRC-protected, sequence-numbered checkpoint only
when the relay state changes or a bounded interval has elapsed (not every
tick), to bound flash wear. A checkpoint write error is never discarded —
every router loop treats it exactly like every other storage error (open
failure, security-store failure): fail closed, relay forced off, LED forced
on, spin forever with no further flash access. See "Voltage-guard fail-closed
gate" in `firmware/tlsr8258-plug/README.md` for why no Zbit flash-voltage
value is ever fabricated to work around this.

`build.rs` copies the product-selected canonical linker script
(`link/tlsr8258-512k.x` or `link/tlsr8258-1m.x`, matching the selected
product's `FlashLayout`) into `OUT_DIR` — never editing the checked-in
`memory.x`/`link/*.x` sources — unless `TLSR8258_LINKER_SCRIPT` is set (used
by `scripts/tlsr8258-firmware.sh` to point at an explicit, already-staged
script).

See `.github/workflows/build-tc32.yml` for the six-way product matrix CI that
exercises this crate's full build / layout-check / symbol-gate / size-report
path (host CI in `ci.yml` is unaffected, since this crate stays
workspace-excluded), and `firmware/tlsr8258-plug/README.md`'s "Hardware
gates" section for the remaining physical validation boundary.


## Upstream dependency

All `zigbee-rs` crates are pinned to commit
`b97c749a66799dfafe9096bb16e4893f45519d5e`. The pin includes calibrated
Electrical Measurement scaling, restoration of the 48-bit Simple Metering
energy counter, the complete reusable TLSR8258 HAL consumed by the firmware,
the typed device-role model (`zigbee_runtime::role`), and the corrected R22
many-to-one/source-routing implementation required by a parent router, plus
the bounded GSDK-style TCLK exchange and normal coordinator-initiated leave
handling.

**Typed `Router` role.** A mains-powered plug is a genuine parent
`Router`, so both router loops name the `zigbee_runtime::role::Router` role
(`ZigbeeDevice<TelinkMac, Router>`) and construct it through
`DeviceBuilder::build_router_into` — bounded on `zigbee_mac::ParentMacDriver`,
which `TelinkMac` implements — never the default leaf `EndDevice` or the
forwarding-only `RelayRouter`. Upstream splits each role's runtime by static
dispatch: a `Router` monomorphization links the parent/child-serving path and
holds `ParentState`, while the R22 End Device Timeout **client** lifecycle is
owned exclusively by `EndDevice` and is therefore never compiled into these
images. The shared helpers (`router_support::apply_stack_event`,
`ZigbeePlug::configure_default_reporting`) stay generic over the role `R` so
host tests can still exercise the profile through other roles, matching the
upstream reference loop's own role-generic signatures.

**`default-features = false`.** `zigbee-zcl` and `zigbee-runtime` are pulled
with `default-features = false, features = ["router"]`, dropping the upstream
`float32`/`float64` ZCL codec: every smart-plug electrical/metering attribute
is integer-scaled, so the float codec is dead weight here. This mirrors
zigbee-rs's own `examples/telink-tlsr8258-router` wiring. `constrained-memory`
is deliberately **not** enabled — a mains-powered parent router must not shrink
any child/route/neighbour table to save flash.

BL0942's raw `CF_CNT` is only 24 bits. Firmware must pass it through
`bl0942::EnergyTracker`, persist `total_uwh` in an `EnergyRecord`, and restore
that lifetime total after reboot. The raw counter-derived reading is never
published directly as Zigbee `CurrentSummationDelivered`.

## Flash partitioning and persistence

`zigbee-plug-hardware::FlashLayout` is the single Rust-side catalog of every
TLSR8258 product's flash partitions (`TLSR8258_512K_LAYOUT`,
`TLSR8258_1M_LAYOUT`): firmware ends before `0x72000`; the product-owned
application-NV log occupies `0x72000..0x74000`; the Zigbee security-counter
journal occupies `0x74000..0x76000`; factory/read-only data begins at
`0x76000` (512 KiB parts) or `0xFE000` (1 MiB parts). 1 MiB parts also
document a `0x96000..0xFC000` candidate energy-journal region that stays
disabled and read-only — no product may treat it as available flash until it
has its own verified staging, identity, and activation policy.

`zigbee-plug-storage` turns that catalog into the type-safe mechanism every
product uses:

- Each board crate (`tlsr8258-legacy-bl0937`, `tlsr8258-ts011f-bl0942`)
  exposes its own zero-sized `OnboardFlash` token, constructible only inside
  that board's singleton-gated `BoardResources::take()`.
- `zigbee_plug_storage::split_onboard_flash` consumes that token by value
  once and returns disjoint `AppNvPartition`/`SecurityPartition` tokens.
  Because the board token cannot be cloned and each partition token is
  itself consumed exactly once, a product cannot construct two overlapping
  raw-flash accessors over the same onboard flash.
- `OnboardFlashToken` is sealed so only genuine board tokens satisfy it —
  not an arbitrary caller-supplied zero-sized type.
- The `zigbee_plug_storage::flash` module (compiled only for
  `target_arch = "tc32"`) wires those partition tokens into
  `zigbee_runtime::log_nv::LogStructuredNv` (application NV) and
  `zigbee_runtime::security_journal::SecurityStateJournal` (security
  counters) over `tlsr8258_hal::flash::Tlsr8258Flash`.
- Each product's `src/storage.rs` (tc32-gated) is the product-owned call
  site that picks its board's `OnboardFlash` and its `FlashLayout` constant
  and opens both stores.

The canonical linker scripts in `link/` (`tlsr8258-512k.x`, `tlsr8258-1m.x`)
carry the proven TLSR8258 cache/RAM/RF-DMA layout from
`zigbee-rs`'s `products/tlsr8258-tb04/link/memory.x` unmodified, export the
same partition boundaries as linker symbols (`_app_nv_*`, `_security_nv_*`,
`_factory_data_*`, `_flash_capacity_`, and — 1 MiB only —
`_candidate_energy_*`), and `ASSERT` at link time that the firmware image
ends at or before `_app_nv_start_` and that every partition is disjoint and
ascending. See `link/README.md` for why two shared scripts cover every
product instead of one per product, and how a future firmware crate's
`build.rs` should select between them.

`scripts/tlsr8258-firmware.sh` is a host-side build/check helper (no `flash`
subcommand) that stages the selected canonical script, builds with bounded
release flags, and independently re-verifies the same boundaries plus the
RAM/cache/RF-DMA symbols from the produced ELF via `llvm-nm`. It also runs a
typed-`Router` **symbol gate** on the linked ELF — asserting the parent path
is present (`zigbee_runtime::role::Router`, `handle_child_rejoin_request`,
`nlme_start_router`) and that no leaf/relay code leaked in (the `EndDevice`/
`RelayRouter` roles and every End Device Timeout *client* method are absent) —
and writes a per-product `*.size.json` (image size, the app-NV-start budget,
remaining headroom, partition addresses, and the `.bin` sha256) that
`build-tc32.yml` uploads alongside each image. All six product features have
been compiled, linked, converted to `.bin`, layout-checked, symbol-gated, and
size-reported with the modern-tc32 toolchain. Host tests additionally cover
the storage token split and bounds arithmetic. Physical flash writes,
security-counter durability, and journal rollover remain hardware gates.

## Hardware gates

BL0942 firmware uses a TLSR8258 UART driver for PB1 TX/PB7 RX at 4800 baud,
8 data bits, no parity, one stop bit (`tlsr8258_hal::uart::Uart`, an
upstream API this repository consumes but does not implement).

BL0937 firmware uses two edge-capture channels (`tlsr8258_hal::capture`):
PB5/PB6 on `legacy-bl0937-pd6`, or PB4/PB5 on
`zbeacon-ts011f-512k`. Both count rising edges in fixed 1 s windows and feed
`bl0937::Bl0937`; a software capture overflow is reported explicitly
(`bl0937_task::Outcome::OverflowDropped`) rather than silently dropped or
interpolated.

Both APIs, and the TLSR8258 timer/IRQ/ADC internals they depend on, are in
the published `zigbee-rs` commit pinned by this repository. No local
`[patch]` is required. Host CI resolves the same revision, while the TC32
workflow compiler-verifies the complete UART/capture/ADC firmware paths for
every product.

### Factory-identity gate for non-512 KiB products

`firmware/tlsr8258-plug/src/router_support.rs`'s `mac_for_product` closes
the multi-geometry identity risk at runtime: it resolves
`FlashGeometry::from_capacity(product_flash_capacity)`, then calls
`factory_ieee_for(geometry, &mut address)` (which itself verifies the
JEDEC ID against the requested geometry before reading, returning
`Err(FlashError)` on mismatch) and `TelinkMac::new_for_flash_geometry`,
returning `None` — treated as a fail-closed startup failure by both
`bl0942_app.rs` and `bl0937_app.rs` — for any unsupported capacity or
geometry/JEDEC mismatch. There is no per-product-family byte offset
applied to the resolved address (see "No EUI mutation" below); the
factory/flash-UID-derived EUI-64 is used unchanged, and is also what
`reset_security_state_if_identity_changed` compares against previously
persisted state.

All six products build reproducibly against the pinned upstream commit.
The four 1 MiB products' post-link layout check reports
`factory_data=[0xFE000..0x100000)`, not the 512 KiB sector. The corresponding
ADC-calibration address is active:
`router_support::install_flash_voltage_guard` now calls
`tlsr8258_hal::adc::Adc::new(adc, geometry)` (which loads the matching
factory ADC calibration for the product's geometry) before installing the
real Zbit flash-voltage guard. The remaining gate is electrical: PC5's
actual connection to a meaningful voltage-sense node on any of the three
physical board families has not been confirmed by schematic or measurement.

### No EUI mutation

Earlier drafts added a per-product-family byte offset
(`wrapping_add(0x42)`/`0x37`) to the resolved factory EUI-64's first
octet, intended to avoid address collisions between differently-reflashed
firmware images. This was removed: only one firmware image ever runs on
a given physical part at a time, so there is no real collision to avoid,
and the mutation corrupts OUI/U-L-bit structure and forces a spurious new
Zigbee identity on every reflash. The factory/flash-UID-derived address
is used unchanged.

OTA remains disabled until each flash geometry has a verified dump,
bootloader activation path, staging partition, and unique image identity.
