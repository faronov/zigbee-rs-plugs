# Canonical TLSR8258 linker scripts

Two scripts cover every TLSR8258 product in this workspace:

| Script | Flash capacity | Products |
|---|---|---|
| `tlsr8258-512k.x` | 512 KiB | `tz3000-gjnozsaz-512k` |
| `tlsr8258-1m.x` | 1 MiB | `legacy-bl0937-pd6`, `tz3000-gjnozsaz-1m`, `tz3000-w0qqde0g`, `tz3000-zloso4jk` |

Both scripts carry over the proven TLSR8258 cache/RAM/RF-DMA layout from
zigbee-rs's `products/tlsr8258-tb04/link/memory.x` unmodified, and additionally
export the product-owned flash partition boundaries as linker symbols so a
firmware crate's post-link check (see `scripts/tlsr8258-firmware.sh`) can
verify them without re-deriving the addresses:

- `_app_nv_start_` / `_app_nv_end_` — application-owned log NV (`0x72000..0x74000`)
- `_security_nv_start_` / `_security_nv_end_` — Zigbee security-counter journal (`0x74000..0x76000`)
- `_factory_data_start_` / `_factory_data_end_` — documented factory/read-only data
- `_flash_capacity_` — total flash size for the target part
- `tlsr8258-1m.x` only: `_candidate_energy_start_` / `_candidate_energy_end_` —
  the disabled, unproven `0x96000..0xFC000` region; no RAM, MEMORY space, or
  write path is reserved for it anywhere in this repository

Both scripts assert at link time that the firmware image ends at or before
`_app_nv_start_` and that every partition is disjoint and in ascending order.

These addresses must stay in sync with
`zigbee_plug_hardware::{TLSR8258_512K_LAYOUT, TLSR8258_1M_LAYOUT}`.
`zigbee-plug-storage` host tests cross-check every exported partition symbol
against those Rust constants.

## Why two shared scripts instead of five per-product copies

Every TLSR8258 product in this workspace differs only in *which flash size it
targets*, not in cache/RAM/RF-DMA layout or in the application-NV/security
partition addresses. Forking a `link/memory.x` per product (as
`zigbee-rs`'s single-product `products/tlsr8258-tb04` does) would mean five
near-identical files that can silently drift apart. Keeping exactly one
script per flash size here, and selecting between them per firmware crate,
keeps that one source of truth.

## Selecting a script from the firmware crate

`firmware/tlsr8258-plug/build.rs` copies the model-appropriate canonical
script to `OUT_DIR/memory.x` and adds it to the link search path. It selects
the 512 KiB script only for `tz3000-gjnozsaz-512k`; the other four product
features select the 1 MiB script. This mirrors the pattern already proven in
`zigbee-rs`'s `examples/telink-tlsr8258-radio/build.rs`:

```rust
fn main() {
    println!("cargo:rustc-check-cfg=cfg(target_arch, values(\"tc32\"))");

    // Host-side `cargo test`/`cargo check` build this same script for the
    // host target; the tc32 linker script must never reach the host linker.
    if std::env::var("TARGET").unwrap_or_default() != "tc32-unknown-none-elf" {
        return;
    }

    // The repository build helper supplies an explicit canonical script.
    // A direct Cargo invocation falls back to the selected product layout.
    let script = std::env::var_os("TLSR8258_LINKER_SCRIPT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_script_for_selected_product);

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::copy(&script, out_dir.join("memory.x")).expect("copy canonical linker script");
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg=-Tmemory.x");
    println!("cargo:rustc-link-arg=--gc-sections");
}
```

`scripts/tlsr8258-firmware.sh` passes the selected canonical path through
`TLSR8258_LINKER_SCRIPT`; `build.rs` performs the only copy, into `OUT_DIR`,
so a build never modifies the source tree.
