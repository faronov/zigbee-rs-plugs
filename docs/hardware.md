# Hardware catalog

## TS011F BL0942 board

Documented connections:

| Function | TLSR8258 pin | Electrical behavior |
|---|---|---|
| Relay | PC2 | active high |
| Status LED | PB4 | active low |
| Button | PB5 | active low, 10 kOhm pull-up |
| BL0942 TX | PB1 | UART TX |
| BL0942 RX | PB7 | UART RX |

Known manufacturer fingerprints are `_TZ3000_w0qqde0g`,
`_TZ3000_gjnozsaz`, and `_TZ3000_zloso4jk`. They do not prove PCB identity:
the same fingerprint has appeared on incompatible hardware.

`_TZ3000_gjnozsaz` needs separate 512 KiB and 1 MiB product targets. The
small-flash stock OTA fingerprint is manufacturer `0x1286`, image type
`0x0002`; the documented large-flash update fingerprint is manufacturer
`0x1141`, image type `0xD3A3`. These values are catalog data only and are not
advertised by this project.

## Legacy BL0937 board

The old experimental pin map contains:

| Function | TLSR8258 pin | Electrical behavior |
|---|---|---|
| Relay | PD6 | active high |
| LEDs | PD7, PD5, PD4 | active high |
| Button | PD3 | raw high assumed pressed |
| BL0937 CF | PB5 | pulse input |
| BL0937 CF1 | PB6 | multiplexed pulse input |
| BL0937 SEL | PB7 | low=current, high=voltage assumption |

No trustworthy Tuya manufacturer fingerprint is tied to this map. It remains
the explicitly named `legacy-bl0937-pd6` target until a PCB marking, flash
dump, and physical inspection establish a real product identity.

## Zbeacon TS011F BL0937 board

A user-owned complete stock flash dump identifies this as a separate
`Zbeacon` / `TS011F` 512 KiB product, not the BL0942 UART board:

| Function | TLSR8258 pin | Electrical behavior |
|---|---|---|
| Relay | PD2 | active high |
| Status LED | PB1 | active low |
| Button | PA0 | active low, 10 kOhm pull-up |
| BL0937 CF | PB4 | rising-edge pulse input, 10 kOhm pull-up |
| BL0937 CF1 | PB5 | rising-edge multiplexed pulse input, 10 kOhm pull-up |
| BL0937 SEL | PD3 | output, stock firmware initializes low |

The stock dump is exactly `0x80000` bytes. Its Telink firmware-length field
is `0x2DD64` (187,748 bytes), ending exactly at the last non-erased byte.
The configured factory sectors (`0x76000`/`0x77000`) and NV map also match
the Telink 512 KiB layout; the true fitted JEDEC capacity still requires a
live flash-ID read. The `b97c749` Rust image is 366,476 bytes, leaves
100,468 bytes before application NV at `0x72000`, and has SHA-256
`dde692569cd541cbefeabdbd5112a3fddcb940b1623a56f71f9da76c89877e7c`.

Useful stock flash regions:

| Range | Stock contents | Replacement consequence |
|---|---|---|
| `0x34000..0x3D000` | Zigbee network/security/table NV | overwritten by the larger Rust image; re-pairing is required |
| `0x40000..0x74000` | erased OTA staging bank | available only after a verified OTA design |
| `0x74000` sector | metering calibration/protection config | replaced by the Rust security journal |
| `0x76000` sector | factory EUI-64 | preserved and used unchanged |
| `0x77000` sector | factory calibration reservation, erased on this unit | preserved |
| `0x7C000` sector | stock binding/reporting NV | left outside Rust partitions, but lost by a full-chip erase |

The image contains live Zigbee identity/security state and therefore is
neither stored nor published by this repository.

The stock endpoint is HA profile `0x0104`, Smart Plug device `0x0051`.
It advertises Basic, Identify, Groups, Scenes, OnOff, Time, Tuya `0xE000`,
Metering `0x0702`, Electrical Measurement `0x0B04`, and Touchlink `0x1000`
as inputs, plus OTA `0x0019` as an output. It also exposes standard
`StartUpOnOff` (`0x4003`) and Tuya child-lock/indicator/startup attributes
`0x8000..0x8002`; these are useful compatibility targets but are not all
implemented by the current Rust profile.

The stock app-config sector contains unit-specific BL0937 values:

| Quantity | Float gain | Integer scaler |
|---|---:|---:|
| RMS current | 16.162 | 200 |
| RMS voltage | 39.136 | 5 |
| Active power | 23.532 | 36 |

Protection limits are 75 V undervoltage, 270 V overvoltage, and 20.5 A
overcurrent; disassembly confirms each trip opens the relay. The Zbeacon
product now supplies these three thresholds to the Rust protection engine.
The recovered stock record contains no power threshold, so the product
disables the generic 3.68 kW limit rather than inventing one. The 5 s trip
delay and disabled voltage auto-restart remain Rust policy because the stock
debounce/restart timing has not been recovered.

The Rust product now converts the exact stock gain/scaler bit patterns into
fixed-point `bl0937::Scale` values without linking soft-float code. It also
uses the statically recovered SEL mapping (`HIGH = voltage`, `LOW = current`)
and derives `2,353,211` CF pulses/kWh from the stock power gain. Metering
remains experimental until SEL timing and all three quantities are checked
against the physical PCB and a known load.

## Candidate models

`_TZ3210_w0qqde0g` has community reports of compatibility with the BL0942
board but is not promoted to a supported target. `_TZ3210_cehuw1lw` has no
verified metering IC or pin map.

## Flash geometry

Both flash sizes share the same firmware/application-NV/security-journal
boundaries; only the factory-data region (and, on 1 MiB parts, an unproven
candidate energy-journal region) moves. See
`zigbee_plug_hardware::{TLSR8258_512K_LAYOUT, TLSR8258_1M_LAYOUT}` for the
authoritative addresses, `zigbee-plug-storage` for the type-safe flash-access
mechanism built on them, and `link/README.md` for the matching canonical
linker scripts. All six product targets compile and link; none has been verified
against a real TLSR8258 plug flash chip.
