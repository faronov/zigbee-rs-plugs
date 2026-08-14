# `tlsr8258-plug` firmware

`no_std`/`no_main` TLSR8258 smart-plug firmware: **one binary, exactly one
compile-time product feature**. See `src/main.rs`'s `compile_error!` guards —
a build with zero or more than one of the six product features fails at
compile time rather than silently picking one or linking two products'
worth of persistence/flash-layout assumptions into one image.

**EXPERIMENTAL. No image built from this crate has been run on TLSR8258
hardware.** See [`docs/safety.md`](../../docs/safety.md) at the workspace
root before flashing anything, and the "Hardware gates" section below for
exactly what is and is not proven.

## Product features

Exactly one of:

| Feature | Board | Metering | Linker layout |
|---|---|---|---|
| `tz3000-gjnozsaz-1m` | `tlsr8258-ts011f-bl0942` | BL0942 (UART) | 1 MiB |
| `tz3000-gjnozsaz-512k` | `tlsr8258-ts011f-bl0942` | BL0942 (UART) | 512 KiB |
| `tz3000-w0qqde0g` | `tlsr8258-ts011f-bl0942` | BL0942 (UART) | 1 MiB |
| `tz3000-zloso4jk` | `tlsr8258-ts011f-bl0942` | BL0942 (UART) | 1 MiB |
| `legacy-bl0937-pd6` | `tlsr8258-legacy-bl0937` | BL0937 (pulse capture) | 1 MiB |
| `zbeacon-ts011f-512k` | `tlsr8258-zbeacon-ts011f-bl0937` | BL0937 (pulse capture) | 512 KiB |

The four BL0942 products share one board crate and one router loop
(`src/bl0942_app.rs`). The two incompatible BL0937 boards share only the
capture-based router loop (`src/bl0937_app.rs`); each retains its own board
and product crate. Model/manufacturer identity and flash layout always come
from the selected product crate (`products/*`); this firmware never infers
a product by runtime fingerprint.

## Building

Use the workspace's `scripts/tlsr8258-firmware.sh`, which requires the
modern-tc32 toolchain (see that script's own `--help`/usage text) and never
flashes a device:

```bash
# check only (fast, no objcopy/layout check)
scripts/tlsr8258-firmware.sh check firmware/tlsr8258-plug tlsr8258-plug 1m tz3000-gjnozsaz-1m

# full build: cargo rustc, objcopy to .bin, post-link layout check
scripts/tlsr8258-firmware.sh build firmware/tlsr8258-plug tlsr8258-plug 512k tz3000-gjnozsaz-512k
scripts/tlsr8258-firmware.sh build firmware/tlsr8258-plug tlsr8258-plug 1m legacy-bl0937-pd6
scripts/tlsr8258-firmware.sh build firmware/tlsr8258-plug tlsr8258-plug 512k zbeacon-ts011f-512k
```

The layout (`512k`/`1m`) must match the selected product's flash geometry
(see the table above and `link/README.md`); `build.rs` also picks a matching
default if `TLSR8258_LINKER_SCRIPT` is left unset by a direct
`cargo build --no-default-features --features <product>` invocation.

`.github/workflows/build-tc32.yml` runs the `build` command above for all
six product features on every push/PR touching this crate or its
dependencies, uploading each `.bin` as an experimental artifact. It never
flashes hardware and has no `flash` job. The measurements below identify the
current macOS `tc32-45` validation build. Linux GitHub Actions produces the
same sizes and layout results, but its output is not asserted to be
byte-identical; use each build's generated `*.size.json` for its own hash.

## Upstream dependency

The firmware pins all `zigbee-rs` crates to commit
`f15ead022537ab9bdd96f0c4b96e352e58a04b1e`. That published revision
contains the reusable TLSR8258 UART, GPIO capture, ADC, flash geometry,
voltage guard, IRQ, timer, and router support used here, plus the typed
device-role model (`zigbee_runtime::role`), corrected Telink factory EUI
decoding, key-bound APS replay/retry handling, persisted R22 rejoin selection,
and complete router Link Status, Network Report/Update, and authenticated
address-conflict maintenance. It retains the bounded GSDK-style TCLK exchange,
normal coordinator-initiated leave handling, priority-aware RX queues, and the
hardware-proven R22 Verify-Key correction described below. No local Cargo
`[patch]` is required.

Updating the stack pin does not repair a stale Trust Center replay floor. A
stock-to-Rust migration keeps the same factory EUI-64 but replaces the stock
app config at `0x74000` with a new zigbee-rs security journal whose APS counter
bounds initially start at zero. If the coordinator already knows that EUI from
the stock firmware, remove the old device/link-key entry before pairing. Once
zigbee-rs has commissioned, never raw-erase `0x74000..0x76000`: factory reset
must preserve the counter bounds, and any manual recovery must restore a
credential-free record with bounds above the Trust Center's retained replay
floor.

For ZiGate coordinators, first verify the radio firmware itself. ZiGate
`v3.1d` used `bSetTclkFlashFeature || u8Status == 1` in the Trust Center
join/rejoin callback `APP_bSendHATransportKey`; `v3.1e` changed the condition
to `&&`, and its release notes identify the change as
`Fix HATransportKey function (Device Authentification)`. Current `v3.23`
retains the corrected condition. ZiGate also requires TCLK exchange and gives
a newly joined node 15 seconds to complete it. The pinned stack starts after
300 ms, keeps independent three-transmission budgets for Node Descriptor,
Request-Key, and Verify-Key, uses 1.5/3/5-second response windows, and enforces
a strict 15-second first-pass deadline.

R22 requires Verify-Key to remain NWK-secured but **not** APS-encrypted. The
pinned stack therefore emits APS frame control `0x41` with no APS auxiliary
security header or MIC, computes the hash from the installed unique TCLK
without consuming its outgoing APS security counter, and treats an APS ACK as
transport feedback only. Commissioning completes only after an authenticated
successful Confirm-Key; missing or rejected Confirm-Key follows the bounded
retry/failure policy instead of an ACK-only compatibility path.

The shared reference router has hardware-proven this exact exchange against
ZiGate v3.23 on the first association: ZiGate validated the `0x41` Verify-Key
and returned an APS-secured successful Confirm-Key. These plug images compile
and link the same shared APS/BDB path, but still require their own board-level
hardware acceptance. Query ZiGate version with command `0x0010` / response
`0x8010`, use at least `v3.1e`, and capture Request-Key, Transport-Key,
Verify-Key, Confirm-Key, and any Leave before changing crypto behavior.
Accepting Confirm-Key under the public `ZigBeeAlliance09` key remains an
unjustified authentication downgrade.

Because a mains plug is a genuine parent, every image is built as a typed
`zigbee_runtime::role::Router` (`ZigbeeDevice<TelinkMac, Router>` via
`build_router_into`), never the default leaf `EndDevice`. `zigbee-zcl` and
`zigbee-runtime` use `default-features = false, features = ["router"]` to
drop the unused `float32`/`float64` ZCL codec (all plug attributes are
integer-scaled); `constrained-memory` is intentionally left off so no
parent/child table is shrunk. See `docs/architecture.md` for the full
rationale.

Hardware AES is mandatory in every production plug image. Each board owns the
exclusive TLSR8258 AES peripheral token, and the selected application installs
it into `TelinkMac` before opening persistence or starting Zigbee. Installation
runs the upstream on-chip known-answer self-test; failure leaves the relay off
and enters the existing fail-closed startup path. There is no runtime software
fallback. The `aes` crate can still appear in `Cargo.lock` as dependency
metadata, so the release gate checks the linked ELF instead: `HardwareAes128`
and `install_aes_engine` must be present, while `aes::soft` and
`SoftwareAes128` must be absent.

## Hardware gates

All six product features compile, link, produce `.bin` files, pass the
post-link flash/RAM/cache/RF-DMA checks, pass the typed-`Router` symbol gate
(hardware AES installed; software AES absent; parent path present; End Device
Timeout client + `EndDevice`/`RelayRouter` roles absent), and emit a
`*.size.json` size/budget report with the modern-tc32 toolchain. Nothing in
this crate has run on physical TLSR8258 plug hardware. The open gates are
therefore:

| Product | Hardware-AES image | Headroom before `0x72000` | SHA-256 |
|---|---:|---:|---|
| `tz3000-gjnozsaz-1m` | 343,612 B | 123,332 B | `a276cc239bd314e16b4ed094cbb26635d77fdcc6fdecaff587ee95c55e0576bc` |
| `tz3000-gjnozsaz-512k` | 343,608 B | 123,336 B | `a098e66a8d58ead459d58eb32ab54f16b9dbafd090dcc37ccd70e04e27212a51` |
| `tz3000-w0qqde0g` | 343,612 B | 123,332 B | `2db4d22f04e8f2ef7fd0b6b52eeaf43a0008f5938cd4f7e4679b282bf146dda7` |
| `tz3000-zloso4jk` | 343,612 B | 123,332 B | `d28242e1b79eaf1963bb09bea30c6634cf72c57621b461257cff05df9e747ba1` |
| `legacy-bl0937-pd6` | 349,292 B | 117,652 B | `76e4174fff1babf9a06fbffc822b5ef86d224785d43845aabea6a1a01a01f82d` |
| `zbeacon-ts011f-512k` | 348,524 B | 118,420 B | `62f8dae70d8172a603f54ade9e677313508dc744485f6e8c4947190a0b99c557` |

These measurements use the pinned `tc32-45` toolchain. Updating the shared
stack from `32836f8` to `f15ead0` adds 12,724-12,728 bytes for the accumulated
R22 replay, persisted-rejoin, Link Status, Network Report/Update,
address-conflict, and exact reporting-completion behavior. No parent/router
table is reduced.

1. preserve and inspect each exact board's original flash;
2. verify JEDEC geometry and PC5 voltage-sense wiring;
3. prove relay, LED, button, BL0942 UART or BL0937 pulse inputs, and flash
   persistence without a connected mains load;
4. prove Zigbee commissioning, reporting, reset/resume, and counter
   durability;
5. keep OTA disabled until each geometry has a verified staging/activation
   layout.

## Factory-identity gate for non-512 KiB products (runtime, geometry-aware, fail-closed)

The pinned HAL exposes `tlsr8258_hal::flash::FlashGeometry`,
`factory_ieee_for` (which verifies JEDEC capacity before reading), and
`zigbee_mac::telink::TelinkMac::new_for_flash_geometry`. Every product
wires its declared flash capacity through those APIs:

```rust
// firmware/tlsr8258-plug/src/router_support.rs
pub fn mac_for_product(flash_capacity: u32) -> Option<(TelinkMac, [u8; 8])> {
    let geometry = FlashGeometry::from_capacity(flash_capacity as usize)?;
    let mut ieee_address = [0u8; 8];
    tlsr8258_hal::flash::factory_ieee_for(geometry, &mut ieee_address).ok()?;
    let mac = TelinkMac::new_for_flash_geometry(geometry).ok()?;
    Some((mac, ieee_address))
}
```

Both `bl0942_app.rs` and `bl0937_app.rs` call this with
`product::PRODUCT.flash.capacity` (never a hardcoded literal) and treat
`None` as a fail-closed startup failure (`fail(&relay, &led)`), covering
both "capacity has no known geometry" and "JEDEC ID doesn't match the
geometry the product claims" — there is no silent fallback to a wrong
sector. The resolved `[u8; 8]` factory EUI-64 is also returned (alongside
the constructed `TelinkMac`, which exposes no public getter for it) and
used unchanged for `reset_security_state_if_identity_changed` — no
per-product byte offset is added to it (see "No EUI mutation" below).

Product metadata/layout itself (`ProductProfile.flash` in each
`products/*/src/lib.rs`) is correct and unaffected: all six products
already declare the right `FlashLayout` for their real flash size
(verified against `plug-hardware`'s `TLSR8258_512K_LAYOUT`/
`TLSR8258_1M_LAYOUT` constants and each product's `validate()` const
assertion).

**Build evidence:** all six products — `tz3000-gjnozsaz-512k` and
`zbeacon-ts011f-512k` (512 KiB geometry), plus `tz3000-gjnozsaz-1m`,
`tz3000-w0qqde0g`, `tz3000-zloso4jk`, and `legacy-bl0937-pd6` (1 MiB
geometry) — build against the pinned upstream commit via
`scripts/tlsr8258-firmware.sh build` (compiles, links, `objcopy`s to
`.bin`, passes the script's post-link layout/RAM/RF-DMA boundary check and
the typed-`Router` symbol gate, and writes `*.size.json`). Current image
sizes are listed above; all remain under the app-NV budget at `0x72000`
(466,944 B), with at least 130,376 bytes of headroom. The 1 MiB builds'
layout-check output
correctly reports `factory_data=[0xFE000..0x100000)`, confirming the
geometry-aware path resolves the right sector rather than the 512 KiB
one. This remains a software/build result, not hardware proof.

## No EUI mutation

Earlier drafts of this firmware added a per-product-family byte offset
(`wrapping_add(0x42)` for BL0942 products, `0x37` for BL0937) to the
first octet of the resolved factory/flash-UID EUI-64, intended to avoid
address collisions between differently-reflashed firmware images on the
same physical part. This has been removed: only one firmware image ever
runs on a given physical TLSR8258 part at a time, so there is no real
collision to avoid, and mutating an arbitrary octet corrupts the
EUI-64's OUI/U-L-bit structure and forces a spurious new Zigbee network
identity on every reflash for no benefit. The factory/flash-UID-derived
address from `mac_for_product` above is used completely unchanged.

## Voltage-guard fail-closed gate

`tlsr8258-hal::flash` gates program/erase operations on Zbit-branded flash
parts (`ZB25WD40B`/`ZB25WD80B`, JEDEC MID `0x13325E`/`0x14325E`) behind an
ADC-backed `VoltageGuardFn`. Both router loops now install a real guard
before opening or writing any persistent storage
(`router_support::install_flash_voltage_guard`, called from
`bl0942_app.rs`/`bl0937_app.rs` right after `mac_for_product` and before
`product::storage::open_storage`):

```rust
pub fn install_flash_voltage_guard(
    adc: tlsr8258_hal::peripherals::Adc,
    pin: tlsr8258_hal::gpio::Pin,
    flash_capacity: u32,
) -> bool {
    let geometry = match FlashGeometry::from_capacity(flash_capacity as usize) {
        Some(geometry) => geometry,
        None => return false,
    };
    match tlsr8258_hal::adc::Adc::new(adc, geometry) {
        Ok(adc) => adc.install_flash_voltage_guard(pin).is_ok(),
        Err(_) => false,
    }
}
```

`Adc::new` verifies the fitted flash's JEDEC ID against the product's
declared geometry (loading the matching factory ADC calibration) before
`install_flash_voltage_guard` takes an output-high PC5 measurement and
registers it as the real `VoltageGuardFn` — the same physical measurement
path Telink's own SDK uses (an otherwise-unused GPIO pad driven high and
sampled as a VBAT sense source), not a fabricated or constant reading.
All three board crates (`tlsr8258-ts011f-bl0942`,
`tlsr8258-legacy-bl0937`, and `tlsr8258-zbeacon-ts011f-bl0937`) now
expose an `adc: tlsr8258_hal::peripherals::Adc` token and a
`flash_voltage_pin: Pin` (PC5, previously unused on all three boards, verified
by grep before reservation) field on `BoardResources`. These resources are
unconditional because the pinned HAL now provides the complete ADC API, so
ordinary host CI compiles the same board ownership surface used by firmware.

If guard installation fails for any reason (unsupported capacity, JEDEC/
geometry mismatch, or an ADC hardware-initialization error), both router
loops treat that as a fail-closed startup failure (the same `fail()` path
used for `mac_for_product` failures) rather than opening storage without
a guard "just in case" the fitted part is not actually Zbit-branded — the
HAL's own `ensure_safe_flash()` already handles the non-Zbit case
safely on its own (`Ok(())` unconditionally, no guard required), so this
firmware only needs the guard to exist correctly when it might matter, not
to special-case flash brand detection itself. There is still no code path
anywhere in this crate that fabricates a fixed voltage reading (e.g. a
constant 3300 mV).

**Build evidence:** all six products build end-to-end against the pinned
upstream revision with this guard wired in — see the note above; the same
`scripts/tlsr8258-firmware.sh build` run that verified the geometry-aware
identity path also verified this. This is still not hardware-proven:
whether PC5 actually senses a meaningful voltage (or floats/is tied to
something else) on the physical `TS011F`/legacy `PD6` boards has not been
confirmed against a schematic or measured hardware, only that the
software path Telink's own SDK uses for this measurement compiles and
encodes the same register sequence.

Voltage-based *protection-engine* trips (`TripReason::UnderVoltage`/
`OverVoltage` in `zigbee_plug_core`) are a separate, unrelated mechanism:
those come from the BL0942/BL0937 mains measurement chips' own
`voltage_mv` samples (see `bl0942_task.rs`/`bl0937_task.rs`), not from any
Zbit ADC guard, and are wired up normally since every product here is
mains-powered (`PowerSource::MainsSinglePhase`).
