# Architecture

The repository follows the same ownership split as `zigbee-rs`:

1. Metering drivers accept bytes or pulse counts and have no board knowledge.
2. Board crates own physical GPIOs and encode active levels.
3. Product crates select one board, flash geometry, and stock identity.
4. `zigbee-plug-core` owns safety and persistence policy.
5. `zigbee-plug-profile` maps validated measurements into standard Zigbee
   clusters.
6. A future firmware crate will compose exactly one product target.

Product selection is intentionally compile-time. Runtime guessing between
different relay pins, metering ICs, or flash layouts can energize a relay or
erase factory data on the wrong PCB.

## Upstream dependency

All `zigbee-rs` crates are pinned to commit
`1c79264a3014ec0a442ccae3860306605d1cdb90`. The pin includes calibrated
Electrical Measurement scaling and restoration of the 48-bit Simple Metering
energy counter.

BL0942's raw `CF_CNT` is only 24 bits. Firmware must pass it through
`bl0942::EnergyTracker`, persist `total_uwh` in an `EnergyRecord`, and restore
that lifetime total after reboot. The raw counter-derived reading is never
published directly as Zigbee `CurrentSummationDelivered`.

## Hardware gates

BL0942 firmware requires a TLSR8258 UART driver for PB1 TX/PB7 RX at 4800
baud, 8 data bits, no parity, and one stop bit.

BL0937 firmware requires two reliable pulse inputs. The ISR must timestamp CF
and CF1 without delaying the radio ISR, handle timer wrap, and expose missed
or overflowed pulses.

OTA remains disabled until each flash geometry has a verified dump,
bootloader activation path, staging partition, and unique image identity.
