//! Selects the canonical TLSR8258 linker script for the enabled product
//! feature and copies it to `OUT_DIR` — this crate's source tree (and its
//! `../../link/*.x` scripts) is never modified by a build.
//!
//! Mirrors zigbee-rs's `examples/telink-tlsr8258-radio/build.rs` (see
//! `link/README.md`'s "Selecting a script from a firmware crate" section for
//! the template this was copied from), extended to pick between the two
//! canonical scripts by product feature instead of hardcoding one.

fn main() {
    // `target_arch = "tc32"` is a real value for the tc32-45 toolchain's
    // built-in `tc32-unknown-none-elf` target, but the host rustc's
    // `check-cfg` doesn't know about it — see the identical comment in the
    // template this was copied from.
    println!("cargo:rustc-check-cfg=cfg(target_arch, values(\"tc32\"))");

    // Host-side `cargo check`/`cargo clippy` (e.g. from an IDE, or a stray
    // invocation without this crate's own `.cargo/config.toml` target
    // override in effect) build this same build script for the host
    // target. The tc32 linker script must never reach the host linker.
    if std::env::var("TARGET").unwrap_or_default() != "tc32-unknown-none-elf" {
        return;
    }

    // The repository build helper (`scripts/tlsr8258-firmware.sh`) supplies
    // an explicit canonical script through this environment variable. A
    // direct `cargo build --features <product>` falls back to selecting the
    // matching canonical script by feature, so the crate remains buildable
    // without the helper script too.
    let script = std::env::var_os("TLSR8258_LINKER_SCRIPT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(default_script_for_selected_product);

    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    std::fs::copy(&script, out_dir.join("memory.x")).expect("copy canonical linker script");
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rerun-if-env-changed=TLSR8258_LINKER_SCRIPT");
    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rustc-link-arg=-Tmemory.x");
    println!("cargo:rustc-link-arg=--gc-sections");
}

/// Only `tz3000-gjnozsaz-512k` targets the 512 KiB part; every other product
/// feature targets the 1 MiB part (see `link/README.md`'s product table).
/// Reads the feature through its `CARGO_FEATURE_*` environment variable
/// (guaranteed available to build scripts) rather than `cfg!(feature = ..)`,
/// which is a less common but equivalent idiom in build scripts.
fn default_script_for_selected_product() -> std::path::PathBuf {
    let script = if std::env::var_os("CARGO_FEATURE_TZ3000_GJNOZSAZ_512K").is_some() {
        "../../link/tlsr8258-512k.x"
    } else {
        "../../link/tlsr8258-1m.x"
    };
    std::path::PathBuf::from(script)
}
