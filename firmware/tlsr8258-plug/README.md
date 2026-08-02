# `tlsr8258-plug` firmware

`no_std`/`no_main` TLSR8258 smart-plug firmware: **one binary, exactly one
compile-time product feature**. See `src/main.rs`'s `compile_error!` guards —
a build with zero or more than one of the five product features fails at
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

The four BL0942 products share one board crate and one router loop
(`src/bl0942_app.rs`); the legacy BL0937 product has its own board crate and
router loop (`src/bl0937_app.rs`). Model/manufacturer identity and flash
layout always come from the selected product crate (`products/*`); this
firmware never infers a product by runtime fingerprint.

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
```

The layout (`512k`/`1m`) must match the selected product's flash geometry
(see the table above and `link/README.md`); `build.rs` also picks a matching
default if `TLSR8258_LINKER_SCRIPT` is left unset by a direct
`cargo build --no-default-features --features <product>` invocation.

`.github/workflows/build-tc32.yml` runs the `build` command above for all
five product features on every push/PR touching this crate or its
dependencies, uploading each `.bin` as an experimental artifact. It never
flashes hardware and has no `flash` job.

## Upstream dependency

The firmware pins all `zigbee-rs` crates to commit
`fd3d13f258a082149f8d77245ef4ebffdef0bdea`. That published revision
contains the reusable TLSR8258 UART, GPIO capture, ADC, flash geometry,
voltage guard, IRQ, timer, and router support used here, plus the typed
device-role model (`zigbee_runtime::role`). No local Cargo `[patch]` is
required.

Because a mains plug is a genuine parent, every image is built as a typed
`zigbee_runtime::role::Router` (`ZigbeeDevice<TelinkMac, Router>` via
`build_router_into`), never the default leaf `EndDevice`. `zigbee-zcl` and
`zigbee-runtime` use `default-features = false, features = ["router"]` to
drop the unused `float32`/`float64` ZCL codec (all plug attributes are
integer-scaled); `constrained-memory` is intentionally left off so no
parent/child table is shrunk. See `docs/architecture.md` for the full
rationale.

## Hardware gates

All five product features compile, link, produce `.bin` files, pass the
post-link flash/RAM/cache/RF-DMA checks, pass the typed-`Router` symbol gate
(parent path present; End Device Timeout client + `EndDevice`/`RelayRouter`
roles absent), and emit a `*.size.json` size/budget report with the
modern-tc32 toolchain. Nothing in this crate has run on physical TLSR8258
plug hardware. The open gates are therefore:

| Product | `59ce930` | `fd3d13f` | Reduction | Current headroom |
|---|---:|---:|---:|---:|
| `tz3000-gjnozsaz-1m` | 362,500 B | 346,596 B | 15,904 B | 120,348 B |
| `tz3000-gjnozsaz-512k` | 362,496 B | 346,592 B | 15,904 B | 120,352 B |
| `tz3000-w0qqde0g` | 362,500 B | 346,596 B | 15,904 B | 120,348 B |
| `tz3000-zloso4jk` | 362,500 B | 346,596 B | 15,904 B | 120,348 B |
| `legacy-bl0937-pd6` | 367,724 B | 351,804 B | 15,920 B | 115,140 B |

The comparison uses the same pinned `tc32-45` toolchain. Most of the reduction
comes from the typed-role runtime update and compiling out the unused
`float32`/`float64` ZCL codec; no parent/router table was reduced.

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
`products/*/src/lib.rs`) is correct and unaffected: all five products
already declare the right `FlashLayout` for their real flash size
(verified against `plug-hardware`'s `TLSR8258_512K_LAYOUT`/
`TLSR8258_1M_LAYOUT` constants and each product's `validate()` const
assertion).

**Build evidence:** all five products — `tz3000-gjnozsaz-512k` (512 KiB
geometry), `tz3000-gjnozsaz-1m`, `tz3000-w0qqde0g`, `tz3000-zloso4jk`, and
`legacy-bl0937-pd6` (all 1 MiB geometry) — build reproducibly against the
pinned upstream commit via
`scripts/tlsr8258-firmware.sh build` (compiles, links, `objcopy`s to
`.bin`, passes the script's post-link layout/RAM/RF-DMA boundary check and
the typed-`Router` symbol gate, and writes `*.size.json`). Current image
sizes are 346,596 B (`tz3000-gjnozsaz-1m`/`-w0qqde0g`/`-zloso4jk`),
346,592 B (`tz3000-gjnozsaz-512k`), and 351,804 B (`legacy-bl0937-pd6`),
all well under the app-NV budget at `0x72000` (466,944 B) — at least
115 KiB of headroom. The 1 MiB builds' layout-check output
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
Both board crates (`tlsr8258-ts011f-bl0942`, `tlsr8258-legacy-bl0937`) now
expose an `adc: tlsr8258_hal::peripherals::Adc` token and a
`flash_voltage_pin: Pin` (PC5, previously unused on both boards, verified
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

**Build evidence:** all five products build end-to-end against the pinned
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
