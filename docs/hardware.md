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
linker scripts. All five layouts compile and link; none has been verified
against a real TLSR8258 plug flash chip.
