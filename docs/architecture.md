# Cross-platform plug architecture

This branch moves the smart-plug lifecycle out of platform firmware and into a
shared, statically composed application model.

The dependency direction is:

```text
application/profile  endpoint, clusters, relay/protection/meter behavior
product              identity, profile/policy selection, layout, persistence
board                fitted pins and exclusive physical resources
platform/chip HAL    clocks, GPIO, timers, flash controller, AES, radio
```

The protocol path remains:

```text
ZigbeePlug profile
        |
plug-router-app
        |
router-app -> zigbee-runtime -> BDB/ZCL/ZDO/APS/NWK/MAC
        |
platform MAC/HAL
```

## Ownership boundaries

### Application and profile

- `zigbee-plug-profile` owns the reusable Smart Plug endpoint and its On/Off,
  Electrical Measurement, and Metering mapping.
- `plug-core` and `plug-controller` own chip-independent settings, protection,
  button/LED behavior, relay reconciliation, and persistent record formats.
- [`apps/plug-router`](../apps/plug-router/) owns the common router lifecycle
  described below. It knows only narrow capabilities (`LocalControl`,
  `MeterService`, `PlugClock`, and `NvStorage`), never GPIO registers or a
  concrete flash device.

### Product

A product contract owns the concrete identity, flash geometry and partitions,
storage journals, application policy/profile selection, calibration, and role
limits. A product may select a reusable shared profile without copying it.
For example:

- each TLSR8258 product crate selects one `FlashLayout`, one board, and opens
  the child/application/security stores over that layout; the composition root
  instantiates the shared `ZigbeePlug` profile, while product-specific
  calibration/protection comes from the product when present;
- `plug-efr32-proof-product` owns the proof identity, always-on End Device
  policy, synthetic development meter, application/security journals, and
  `products/plug-efr32-proof/link/memory.x`.

Shared layout catalogs and journal implementations may be reused, but the
product remains the call site that selects them. A board never chooses a model
string, security policy, or protected partition.

### Board

A board crate describes only fitted hardware and hands out single-owner typed
resources:

- exact relay, LED, button, meter, and voltage-guard pins;
- peripheral ownership tokens such as UART, ADC, AES, clocks, or internal
  flash;
- active levels and conservative initial GPIO configuration.

Board crates do not depend on `zigbee-runtime` or `plug-router-app`. The board
may expose an exclusive physical flash token, but the product decides how that
flash is partitioned and what persistence policy uses it.

### Firmware composition root

Firmware `main.rs`/platform modules:

1. initialize the chip and take the board resources;
2. install the selected MAC/crypto/clock mechanisms;
3. ask the product to construct profile, policy, and storage;
4. construct the typed router frontend and shared plug app;
5. enter the platform event loop.

The firmware roots do not reimplement commissioning, reset, reporting,
protection, metering checkpoints, or child persistence.

## `PlugRouterApp` and `AlwaysOnEndDevicePlugApp`

Both public wrappers in `plug-router-app` own the same private `PlugCore`.
Their only difference is the statically selected network frontend.

### `PlugRouterApp`

`PlugRouterApp` wraps
[`router_app::ParentRouterApp`](https://github.com/faronov/zigbee-rs/blob/1d7df8ffbaa794ecca27c1e082c1c023b0094ca7/apps/router/src/app.rs).
It requires:

- a `ParentMacDriver`;
- the typed `zigbee_runtime::role::Router`;
- a `ChildTableStore`, normally through `PersistentChildren`.

It therefore links child admission/serving and restores a durable child table
before parent service begins. All six TLSR8258 products use this composition.

### `AlwaysOnEndDevicePlugApp`

`AlwaysOnEndDevicePlugApp` wraps
[`router_app::AlwaysOnEndDeviceApp`](https://github.com/faronov/zigbee-rs/blob/1d7df8ffbaa794ecca27c1e082c1c023b0094ca7/apps/router/src/app.rs).
It needs only `MacDriver`, and its role is statically
`zigbee_runtime::role::EndDevice`.

This is a receiver-on-when-idle, mains-powered leaf. It joins and rejoins
through a parent but neither routes nor admits children. The EFR32 proof uses
it because `Efr32s2Mac` does not implement `ParentMacDriver`, avoiding the
previous non-conformant Router claim. Its manifests also leave the
`zigbee-runtime/router` feature disabled, so route, parent, and child-table
capacities are absent rather than merely unused. The product allocates no
child journal.

### Shared `PlugCore` behavior

The private core is the single implementation of:

- application-state restore and startup relay reconciliation;
- local button selection copied into the ZCL OnOff attribute, then
  acknowledged back to the platform service;
- unconditional protection veto after local and network commands;
- startup relay interlock until the first safe electrical sample;
- fail-closed meter-health latching for dropped/reset/stale input;
- the mandatory 100 ms On/Off tick;
- one bounded, nonblocking meter service per application step;
- electrical-sample delivery to the protection engine;
- relay-change or 60-second application-state checkpoints;
- network-status-to-LED mapping;
- synchronous physical relay-off acknowledgement before local or
  network-requested factory-reset persistence/network work;
- deferred network reset commit so application Off is durable before security
  state and the parent child journal are cleared;
- interrupt-owned sticky relay inhibits for long-press and meter deadlines
  while Zigbee futures are awaiting.

The finite `step()` sequence is intentionally observable and host-tested.
Platform-specific Timer1/SysTick code only implements `LocalControl`; it does
not own ZCL or network state.

## Platform compositions

### TLSR8258 parent plugs

`firmware/tlsr8258-plug` selects exactly one of six product features. Its two
family composition modules differ only in fitted meter construction:

- BL0942 products construct PB1/PB7 UART metering;
- BL0937 products construct the appropriate pulse-capture pins and product
  calibration.

Both build `ParentRouterApp + PersistentChildren + PlugRouterApp`. Timer0
provides the application clock; Timer1 provides the independent 10 ms
button/relay/LED and relative meter-watchdog service. The product owns
identity, layout, and three stores.

### EFR32MG21 BRD4181A relay proof

`firmware/plug-efr32-proof` builds:

```text
Efr32s2Mac
  + ZigbeeDevice<_, EndDevice>
  + AlwaysOnEndDeviceApp
  + AlwaysOnEndDevicePlugApp
```

It is configured `PowerMode::AlwaysOn`. The board supplies PD2, PB0,
PC3/EXP10, clocks, and the sole internal-flash token. The product supplies the
identity, shared Smart Plug profile, software-AES startup KAT policy,
synthetic meter, and application/security journals.

This is a compile/link portability proof only. It does not establish EFR32
radio, flash, GPIO, entropy, low-power, mains, or metering behavior on
hardware.

## Persistence partitions

### TLSR8258 products

Both 512 KiB and 1 MiB layouts use the same protected low-flash boundaries:

| Range | Owner | Purpose |
|---|---|---|
| `0x00000..0x70000` | Product firmware image | Image must end **strictly before** `0x70000` |
| `0x70000..0x72000` | Product persistence | Two-sector durable child-table journal |
| `0x72000..0x74000` | Product persistence | Two-sector application NV (`AppEndpoint1` plug state) |
| `0x74000..0x76000` | Product persistence | Zigbee credentials and crash-safe frame-counter bounds |

Geometry-specific protected regions are:

| Geometry | Additional defined regions |
|---|---|
| 512 KiB | factory/read-only `0x76000..0x78000`; the remaining upper flash is not exposed by the current product storage API |
| 1 MiB | disabled candidate region `0x96000..0xFC000`; factory/read-only `0xFE000..0x100000`; gaps are not writable product storage |

The board creates one `OnboardFlash` token. The product consumes it once and
receives disjoint child/application/security partition tokens. The linker
scripts export all boundaries and reject an image at or above the child
journal. The build helper independently checks the ELF, binary size, RAM/cache
layout, and linked journal/router symbols.

### EFR32MG21 proof

`products/plug-efr32-proof/link/memory.x` defines:

| Range | Purpose |
|---|---|
| `0x00000000..0x00004000` | Reserved bootloader area |
| `0x00004000..0x00078000` | Application image, 464 KiB |
| `0x00078000..0x0007C000` | Application journal, two 8 KiB sectors |
| `0x0007C000..0x00080000` | Security journal, two 8 KiB sectors |
| `0x20000000..0x20010000` | 64 KiB SRAM |

There is no child partition because this product is statically an End Device.
The board's one flash-controller token is split into two type-distinguished,
bounds-checked views. The application journal uses 64-byte commit-last
records; host tests cover interrupted append, interrupted rollover, CRC
fallback, and tombstones. Generation wrap fails closed with `NvError::Full`.
The security journal is the shared EFR32 two-sector journal from the core
branch.

## Reset and erase ordering

An urgent four-second local reset is handled before an already-due network
retry:

1. set the ZCL/local desired state to Off and synchronously acknowledge the
   physical relay is Off;
2. durably checkpoint relay Off and current accumulated energy in application
   NV;
3. reset the security state to a credential-free record while preserving the
   global and Trust Center outgoing-counter upper bounds;
4. on `PlugRouterApp`, durably replace the child table with an empty snapshot;
   `AlwaysOnEndDevicePlugApp` has no child store;
5. schedule immediate recommissioning;
6. enter fresh steering only on a subsequent application step.

If the application checkpoint fails, security reset, child clear, and steering
do not start. If security or child persistence fails, steering is not entered.
Both firmware roots treat the returned error as a terminal fault and force the
relay output inactive.

The erase rules follow from that ordering:

- never implement factory reset as a raw erase of the security journal;
  preserving counter bounds prevents key/counter reuse after power loss;
- never erase the child journal as a substitute for an explicit empty durable
  snapshot;
- never erase the application source sector before a replacement record is
  committed in the other sector;
- never erase an EFR32 bootloader region or a TLSR8258 factory/read-only region;
- never full-chip erase a device without first preserving all identity,
  calibration, and rollback-required data.

## Fail-safe output behavior

Every board configures the relay inactive before enabling its output driver.
For BRD4181A, the HAL clears the PC3 latch before changing the pin to
push-pull. For TLSR8258 boards, the board writes the relay's inactive level
before setting output-enable.

Before local-control ownership is initialized, startup failures reset or halt
with the board latch inactive. After initialization, a runtime/persistence
error enters the platform fault path: relay Off, fault LED On, and no further
application progress. A persisted `Previous` startup state is applied only
after storage restore and controller/ZCL startup policy have run.

These are source, host-test, and build properties. Actual pin waveforms,
brownout/reset behavior, relay hardware, and mains safety still require HIL
and electrical validation.

## Experiment-branch dependency

The manifests currently consume the adjacent
[`1d7df8f`](https://github.com/faronov/zigbee-rs/tree/1d7df8ffbaa794ecca27c1e082c1c023b0094ca7)
checkout by relative path. They are not pinned to the older published commit
described by previous documentation.

The public zigbee-rs GitHub Pages book is deployed from the core repository's
main/deploy path. Branch source links above are authoritative for this
migration until that branch is merged and Pages is deployed.
