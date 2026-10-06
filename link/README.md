# Canonical TLSR8258 linker scripts

Two scripts cover all six TLSR8258 products:

| Script | Flash capacity | Products |
|---|---:|---|
| `tlsr8258-512k.x` | 512 KiB | `tz3000-gjnozsaz-512k`, `zbeacon-ts011f-512k` |
| `tlsr8258-1m.x` | 1 MiB | `legacy-bl0937-pd6`, `tz3000-gjnozsaz-1m`, `tz3000-w0qqde0g`, `tz3000-zloso4jk` |

The product crate selects its `FlashLayout`; the firmware feature and
`build.rs` select the matching canonical script. Board crates do not select
memory partitions.

## Exported partitions

Both scripts export:

- `_child_nv_start_` / `_child_nv_end_` — child-table journal,
  `0x70000..0x72000`;
- `_app_nv_start_` / `_app_nv_end_` — application-state log,
  `0x72000..0x74000`;
- `_security_nv_start_` / `_security_nv_end_` — Zigbee security/counter
  journal, `0x74000..0x76000`;
- `_factory_data_start_` / `_factory_data_end_` — geometry-specific
  factory/read-only range;
- `_flash_capacity_` — fitted capacity selected by the product.

The 1 MiB script also exports `_candidate_energy_start_` and
`_candidate_energy_end_` for `0x96000..0xFC000`. Those symbols document a
disabled candidate region only. No `MEMORY` entry or Rust write accessor
exists for it.

The image must end **strictly before** `_child_nv_start_` (`0x70000`).
Ending at the boundary is rejected. The scripts also assert that the
child/application/security/factory regions are disjoint and ascending.

## SRAM/cache layout

Both scripts retain the TLSR8258 cache, RAM-code, data, RF-DMA, SVC stack, and
IRQ stack layout used by the core TLSR8258 product:

```text
0x840000 + A   RAM-code backing end / I-cache tag start
0x840100 + A   I-cache tag end / I-cache data start
0x840900 + A   I-cache data end / writable data start
0x84BC00       SVC stack bottom
0x84FC00       SVC stack top / IRQ stack bottom
0x850000       IRQ stack top / SRAM end
```

`A` is the RAM-code preload size rounded to 256 bytes. Link assertions and
`scripts/tlsr8258-firmware.sh` independently reject cache/data, DMA/stack, or
image/persistence overlap.

## Why scripts are shared by geometry

The six products differ in product identity, board/meter selection, and flash
capacity. Their TLSR8258 cache/RAM and low-flash journal boundaries do not
differ. One script per capacity prevents six near-identical copies from
drifting.

The address catalog in these scripts is cross-checked by host tests against
`zigbee_plug_hardware::{TLSR8258_512K_LAYOUT, TLSR8258_1M_LAYOUT}`.
`zigbee-plug-storage` consumes the board's one flash token and returns three
type-distinguished partition tokens, so safe Rust cannot create overlapping
child/application/security accessors.

## Selection and validation

`firmware/tlsr8258-plug/build.rs` copies the selected script to
`OUT_DIR/memory.x`; builds never modify a checked-in linker file. The
repository helper passes an explicit absolute script through
`TLSR8258_LINKER_SCRIPT`:

```bash
scripts/tlsr8258-firmware.sh build \
  firmware/tlsr8258-plug tlsr8258-plug 512k \
  tz3000-gjnozsaz-512k
```

The helper then:

1. builds with the pinned modern-tc32 `tc32-1.98.1-20261003-31a272` toolchain
   (LLVM tail merging enabled; `TC32_TOOLCHAIN` overrides the directory);
2. converts the ELF to `.bin`;
3. verifies linker symbols, image size, RAM-code/cache/RF-DMA/stack layout;
4. verifies hardware AES and the child-capable typed `Router` path;
5. rejects software AES, `RamChildTableStore`, `EndDevice`, and
   forwarding-only `RelayRouter` leakage;
6. writes a `*.size.json` report using `0x70000` as the image budget.

This is compile/link evidence only. It does not validate program/erase,
journal rollover, reset retention, or any other operation on a fitted flash
chip.
