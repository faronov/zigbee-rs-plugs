# Hardware catalog and proof status

## EFR32MG21 BRD4181A smart-plug proof

The proof composition targets exactly:

- radio board: `BRD4181A`;
- main board: `BRD4001A`;
- MCU: `EFR32MG21A020F512IM32`;
- role: receiver-on-when-idle `EndDevice`, always on and non-routing;
- firmware target: `thumbv8m.main-none-eabihf`.

The source and post-link verifier enforce this proof wiring:

| Resource | Pin/source | Code-defined behavior |
|---|---|---|
| User button | PD2, built-in BTN0 | Active low. Configured as input with `Pull::None` because the BRD4001A supplies the bias. Sampled every 10 ms; 30 ms debounce; short press clears a latched trip, toggles the logical relay while joined, or requests Network Steering while not joined; four-second hold requests factory reset and re-steering. |
| Status LED | PB0, built-in LED0 | Active high push-pull. Off initially; then follows the shared network/relay/fault LED policy. |
| Relay proof output | PC3, WSTK expansion header pin 10 (`EXP10`) | Active high push-pull. The latch is cleared before output mode is enabled. This is only a low-voltage logic proof point; no mains interface is defined. |
| Clock | HFXO | Configured for 38.4 MHz with CTUNE 133. SysTick supplies the 1 kHz local service and a 1 MHz Embassy time base. |
| Storage | Internal MSC flash token | Split once into product-owned application and security partitions. There is no child partition. |

`Efr32s2Mac` is constructed by the firmware after board setup. The proof image
uses `DeviceType::EndDevice`, `PowerMode::AlwaysOn`, and
`AlwaysOnEndDeviceApp` because this MAC does not implement
`ParentMacDriver`. Its dependency graph excludes `zigbee-runtime/router`.

The current product also deliberately uses:

- RustCrypto software AES-128, checked by a startup FIPS-197 known-answer
  vector; this is not EFR32 Secure Element/CRYPTO validation;
- synthetic 230 V / 0 A / 0 W / 50 Hz development samples every five seconds;
  restored energy is retained, but no new energy is invented;
- no watchdog and no low-power policy.

The locked release build and verifier currently report:

| Measurement | Value |
|---|---:|
| Flash payload / linked span | 198,092 B / 198,096 B |
| Application region | 475,136 B (`0x4000..0x78000`) |
| Flash headroom before application journal | 279,232 B |
| Static RAM (`data+bss`) | 18,576 B |
| Static RAM headroom before accounting for stack | 46,964 B |

This is build proof, not lab proof. No BRD4181A GPIO waveform, internal-flash
write, radio packet, association, Zigbee report, reset sequence, current draw,
or long-duration run has been measured by this repository.

### EFR32 hardware-only gates

1. Observe PD2 debounce/short press/four-second reset, PB0 status patterns, and
   PC3 inactive-at-reset behavior on BRD4181A/BRD4001A.
2. Prove PC3/EXP10 remains inactive through power-on, panic reset, storage
   failure, radio failure, and repeated reset.
3. Exercise both 8 KiB application sectors and both 8 KiB security sectors,
   including interrupted append, interrupted rollover, and reset retention.
4. Capture raw IEEE 802.15.4 TX/RX, FCS, ACK timing, scan, association,
   rejoin, receiver-on End Device behavior, and ZCL traffic.
5. Validate entropy and replace or explicitly accept the software AES provider
   before any production security claim.
6. Replace the synthetic meter with a real, calibrated hardware source before
   any metering or protection claim.
7. Add and validate watchdog, brownout, low-power, and long-duration behavior.
8. Design and electrically qualify a mains relay interface separately; PC3 is
   not such an interface.

## TLSR8258 product matrix

| Product feature | Manufacturer/model | Board | Meter | Flash | Evidence in product crate |
|---|---|---|---|---:|---|
| `tz3000-gjnozsaz-1m` | `_TZ3000_gjnozsaz` / `TS011F` | TS011F PC2 | BL0942 UART | 1 MiB | documented |
| `tz3000-gjnozsaz-512k` | `_TZ3000_gjnozsaz` / `TS011F` | TS011F PC2 | BL0942 UART | 512 KiB | experimental |
| `tz3000-w0qqde0g` | `_TZ3000_w0qqde0g` / `TS011F` | TS011F PC2 | BL0942 UART | 1 MiB | documented |
| `tz3000-zloso4jk` | `_TZ3000_zloso4jk` / `TS011F` | TS011F PC2 | BL0942 UART | 1 MiB | documented |
| `legacy-bl0937-pd6` | unknown / `TS011F` | legacy PD6 | BL0937 pulse | 1 MiB | pin map only |
| `zbeacon-ts011f-512k` | `Zbeacon` / `TS011F` | Zbeacon PD2 | BL0937 pulse | 512 KiB | pin map only |

The model and manufacturer fields do not prove the fitted PCB or flash
geometry. `_TZ3000_gjnozsaz` is deliberately split into separate 512 KiB and
1 MiB products.

### TS011F BL0942 board

| Function | TLSR8258 pin | Code-defined behavior |
|---|---|---|
| Relay | PC2 | active high; written low before output enable |
| Status LED | PB4 | active low |
| Button | PB5 | active low, 10 kOhm pull-up |
| BL0942 TX | PB1 | UART TX, 4,800 baud, no parity, one stop bit |
| BL0942 RX | PB7 | UART RX, 4,800 baud, no parity, one stop bit |
| Flash voltage guard | PC5 + ADC | reserved for the geometry-aware Zbit program/erase voltage check |

Known product fingerprints are `_TZ3000_w0qqde0g`, `_TZ3000_gjnozsaz`, and
`_TZ3000_zloso4jk`. They do not establish PCB identity.

### Legacy BL0937 board

| Function | TLSR8258 pin | Code-defined behavior |
|---|---|---|
| Relay | PD6 | active high; written low before output enable |
| Status/aux LEDs | PD7, PD5, PD4 | active high |
| Button | PD3 | raw high is treated as pressed |
| BL0937 CF | PB5 | pulse input |
| BL0937 CF1 | PB6 | multiplexed pulse input |
| BL0937 SEL | PB7 | starts low; product code uses the legacy high-is-current assumption |
| Flash voltage guard | PC5 + ADC | reserved for the geometry-aware Zbit program/erase voltage check |

No trustworthy Tuya manufacturer fingerprint is tied to this map. It remains
explicitly named `legacy-bl0937-pd6`.

### Zbeacon TS011F BL0937 board

| Function | TLSR8258 pin | Code-defined behavior |
|---|---|---|
| Relay | PD2 | active high; written low before output enable |
| Status LED | PB1 | active low |
| Button | PA0 | active low, 10 kOhm pull-up |
| BL0937 CF | PB4 | rising-edge pulse input, 10 kOhm pull-up |
| BL0937 CF1 | PB5 | rising-edge multiplexed pulse input, 10 kOhm pull-up |
| BL0937 SEL | PD3 | output, starts low; product selects high=voltage |
| Flash voltage guard | PC5 + ADC | reserved for the geometry-aware Zbit program/erase voltage check |

A user-owned stock dump is exactly `0x80000` bytes. Its Telink firmware-length
field is `0x2DD64` (187,748 bytes), ending at the last non-erased stock byte.
The dump is not published because it contains live Zigbee identity/security
state.

The current cross-platform build is 373,820 bytes, ends before the child
journal at `0x70000`, leaves 84,932 bytes of image headroom, and has SHA-256
`8c4ab1e149de6eec3909cfcff9cbe2b58a90fc5da92f4837e7c45181f447311e`.
This is a compiler/linker result only.

Relevant stock regions and replacement consequences:

| Range | Stock contents | Current Rust consequence |
|---|---|---|
| `0x34000..0x3D000` | Zigbee network/security/table NV | Overlapped by the larger Rust image; stock network state cannot be resumed |
| `0x40000..0x74000` | erased OTA staging bank | Partly consumed by the Rust image and by child/app journals; not an available OTA slot |
| `0x70000..0x72000` | within the stock staging area | Rust child-table journal |
| `0x72000..0x74000` | within the stock staging area | Rust application-state journal |
| `0x74000..0x76000` | metering calibration/protection configuration | Rust security journal; first Rust commissioning starts a new journal |
| `0x76000..0x77000` | factory EUI-64 sector | Preserved and used unchanged |
| `0x77000..0x78000` | factory calibration reservation, erased on the inspected unit | Preserved |
| `0x7C000..0x7D000` | stock binding/reporting NV | Outside current Rust partitions, but destroyed by a full-chip erase |

The stock endpoint is HA profile `0x0104`, Smart Plug device `0x0051`. It
advertises Basic, Identify, Groups, Scenes, OnOff, Time, Tuya `0xE000`,
Metering `0x0702`, Electrical Measurement `0x0B04`, and Touchlink `0x1000` as
inputs, plus OTA `0x0019` as an output. The Rust profile does not claim every
stock/Tuya extension.

The recovered stock BL0937 values are:

| Quantity | Float gain | Integer scaler |
|---|---:|---:|
| RMS current | 16.162 | 200 |
| RMS voltage | 39.136 | 5 |
| Active power | 23.532 | 36 |

The product converts the exact bit patterns into fixed-point scales and uses
2,353,211 CF pulses/kWh. It configures 75 V undervoltage, 270 V overvoltage,
20.5 A overcurrent, no invented power threshold, a five-second Rust trip
delay, and no voltage auto-restart. Metering and protection remain
hardware-unproven.

## TLSR8258 flash geometry

Every product uses:

| Range | Purpose |
|---|---|
| `0x00000..0x70000` | firmware budget |
| `0x70000..0x72000` | child-table journal |
| `0x72000..0x74000` | application-state journal |
| `0x74000..0x76000` | security/counter journal |

On 512 KiB products, factory/read-only data is
`0x76000..0x78000`. On 1 MiB products it is
`0xFE000..0x100000`; `0x96000..0xFC000` is only a documented, disabled
candidate region and has no write accessor.

All six products compile, link, pass the post-link layout and role/AES symbol
gates, and fit below `0x70000`. None has been exercised against a physical
TLSR8258 plug, flash chip, relay, button, meter, or PC5 voltage-sense node.
See the [firmware README](../firmware/tlsr8258-plug/README.md) for the exact
commands and current size table.

## Candidate models

`_TZ3210_w0qqde0g` has community reports of compatibility with the BL0942
board but is not a product target. `_TZ3210_cehuw1lw` has no verified meter or
pin map.
