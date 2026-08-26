# Safety and destructive-operation boundaries

Smart plugs contain non-isolated mains-voltage circuitry.

- Disconnect a plug before opening it.
- Do not power an exposed plug PCB from mains while attaching a programmer.
- Use an isolated low-voltage programming setup and verify test points before
  connecting it.
- Treat custom overload protection as an additional safeguard, never as a
  certified protective device.

The EFR32 BRD4181A target is different: PC3/EXP10 is only a low-voltage logic
proof output. Do not connect it directly to a mains relay or use the proof
image as a mains-control design.

## Before any flash operation

1. Identify the exact PCB, MCU, meter, relay pin and active level.
2. Read and preserve a complete flash dump.
3. Read the fitted flash JEDEC ID and match it to the product feature.
4. Preserve factory identity, calibration, bootloader, rollback, and network
   state required by the intended recovery plan.
5. Verify the image/linker geometry. A Tuya manufacturer/model string is not
   sufficient evidence.

The two `_TZ3000_gjnozsaz` targets have different flash capacities. Selecting
the wrong one can read or erase the wrong factory sector.

## Protected persistence

TLSR8258 products reserve:

- `0x70000..0x72000` for the child-table journal;
- `0x72000..0x74000` for application state;
- `0x74000..0x76000` for credentials and frame-counter bounds.

The EFR32 proof reserves:

- `0x00000000..0x00004000` for the bootloader;
- `0x00078000..0x0007C000` for application state;
- `0x0007C000..0x00080000` for security state.

Never raw-erase a security journal as a factory reset. The reset path writes a
credential-free record while preserving the global and Trust Center outgoing
counter upper bounds, preventing a later key/counter pair from being reused.

The required local-reset order is:

1. logical relay Off plus synchronous physical relay Off acknowledgement;
2. durable application checkpoint recording relay Off;
3. security reset preserving counter bounds;
4. durable child-table clear on parent-capable TLSR8258 products;
5. recommission scheduling;
6. fresh steering on a later application step.

If the application checkpoint fails, network reset and steering must not
start. Do not replace this sequence with sector erases or a full-chip erase.

For stock-to-Rust migration, remove the old coordinator device/link-key entry
before the first Rust commissioning when the coordinator still remembers the
same factory EUI-64 and a higher replay floor. After Rust commissioning, keep
the security journal intact across ordinary factory reset.

## Fail-safe relay behavior

The board code writes the relay inactive before enabling its output driver.
The shared application reconciles every local/network On request through the
protection latch. Protection trips and reset handling use a synchronous
platform operation that physically opens the relay before flash or network
state changes continue. Firmware panic/fault paths perform the same direct
relay-off write and show the fault LED.

Metering is also fail-closed. The relay remains physically Off until the first
decoded sample is inside the configured protection limits. The default health
policy latches a fault after 10 seconds without the first sample, after 10
seconds without a subsequent sample, or immediately on capture overflow/UART
interface reset. Timer1/SysTick enforces the two time deadlines in interrupt
context, so a scan, association, or rejoin await cannot postpone physical
relay-off. Its inhibit is sticky until the application has consumed the fault
or completed the durable reset transaction. Clearing the latch starts a new
first-sample interlock; it does not energize the relay by itself.

These are source, host-test, and build properties. They do not replace HIL:

- inspect the actual output during power-up, reset, brownout, watchdog/panic,
  flash failure, radio failure, and repeated reboot;
- first test without a connected mains load;
- verify local control during scan, association, retry, rejoin, and joined
  operation;
- unplug or stall the meter interface and prove the relay opens before the
  configured health deadline and stays latched Off;
- measure relay timing and current/voltage protection against calibrated
  equipment before attaching a real load.

GitHub Actions may upload short-lived experimental ELF/BIN/HEX and size
artifacts. No release or OTA image is hardware-qualified or published as a
supported firmware image, and the repository provides no flashing command.
