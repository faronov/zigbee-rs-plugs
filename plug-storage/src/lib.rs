//! Shared TLSR8258 flash-partition and Zigbee persistence wiring.
//!
//! Every TLSR8258 smart-plug product in this workspace shares the same
//! proven partition boundaries recorded in
//! [`zigbee_plug_hardware::FlashLayout`]: firmware ends before `0x70000`,
//! the durable child-table journal occupies `0x70000..0x72000`,
//! the product-owned application-NV log occupies `0x72000..0x74000`, and
//! the Zigbee security-counter journal occupies `0x74000..0x76000`. This
//! crate turns that shared catalog into one reusable, type-safe mechanism
//! instead of duplicating near-identical `NorFlash` wrappers per product.
//!
//! # Ownership model
//!
//! Each board crate ([`tlsr8258_legacy_bl0937`],
//! [`tlsr8258_ts011f_bl0942`], and
//! [`tlsr8258_zbeacon_ts011f_bl0937`]) exposes its own zero-sized `OnboardFlash`
//! token, constructible only inside that board's `BoardResources::take()`
//! (itself gated to succeed at most once per boot by
//! `tlsr8258_hal::peripherals::Peripherals::take`). [`split_onboard_flash`]
//! consumes that single token by value and returns the three disjoint
//! partition tokens ([`ChildTablePartition`], [`AppNvPartition`], and
//! [`SecurityPartition`]) that a product needs to build its child-table,
//! application-NV, and security-journal
//! accessors. Because the board token cannot be cloned or reconstructed,
//! and each partition token is itself consumed exactly once by
//! `flash::AppNvFlash::new`/`flash::SecurityFlash::new` (only compiled for `target_arch = "tc32"`), a product can
//! never safely build two overlapping raw-flash accessors over one board's
//! onboard flash.
//!
//! [`OnboardFlashToken`] is sealed: only the three board crates above may
//! stand in for genuine, singleton-gated onboard flash ownership, so an
//! arbitrary zero-sized value (e.g. `()`) cannot be substituted for it.
//!
//! # Candidate energy-journal region
//!
//! 1 MiB parts additionally expose a `0x96000..0xFC000` region documented
//! by [`zigbee_plug_hardware::TLSR8258_1M_LAYOUT`] as
//! `candidate_energy_journal`. This crate deliberately provides **no**
//! `NorFlash` accessor for it — see [`CandidateEnergyJournal`] — it remains
//! disabled and unproven until a product explicitly adopts it.
//!
//! # Validation status
//!
//! Host-tested only: partition-bounds arithmetic and token-splitting are
//! covered by this crate's unit tests. The `flash` module's `NorFlash`
//! wrappers compile only for `target_arch = "tc32"` and have not been
//! exercised on real TLSR8258 hardware from this crate.
#![no_std]

use zigbee_plug_hardware::{FlashLayout, FlashRegion};

#[cfg(target_arch = "tc32")]
pub mod flash;

mod sealed {
    /// Sealed so only this crate can grant [`super::OnboardFlashToken`].
    pub trait Sealed {}
}

/// Marker for a board's exclusively-owned onboard TLSR8258 flash token.
///
/// Implemented in this crate for each supported board's own zero-sized
/// `OnboardFlash` type. Being sealed means an arbitrary caller cannot
/// fabricate a substitute token merely by naming this trait — only the
/// board crates below, which gate construction behind their singleton
/// `BoardResources::take()`, can produce a value that satisfies it.
pub trait OnboardFlashToken: sealed::Sealed {}

impl sealed::Sealed for tlsr8258_legacy_bl0937::OnboardFlash {}
impl OnboardFlashToken for tlsr8258_legacy_bl0937::OnboardFlash {}

impl sealed::Sealed for tlsr8258_ts011f_bl0942::OnboardFlash {}
impl OnboardFlashToken for tlsr8258_ts011f_bl0942::OnboardFlash {}

impl sealed::Sealed for tlsr8258_zbeacon_ts011f_bl0937::OnboardFlash {}
impl OnboardFlashToken for tlsr8258_zbeacon_ts011f_bl0937::OnboardFlash {}

/// Exclusive right to construct the product's application-NV flash
/// accessor. Produced only by [`split_onboard_flash`] and consumed exactly
/// once by `flash::AppNvFlash::new` (or the `flash::application_nv`
/// helper, only compiled for `target_arch = "tc32"`).
pub struct AppNvPartition(());

/// Exclusive right to construct the Zigbee child-table journal flash
/// accessor. Produced only by [`split_onboard_flash`] and consumed exactly
/// once by `flash::ChildTableFlash::new` (or the
/// `flash::child_table_store` helper, only compiled for `target_arch =
/// "tc32"`).
pub struct ChildTablePartition(());

/// Exclusive right to construct the Zigbee security-journal flash
/// accessor. Produced only by [`split_onboard_flash`] and consumed exactly
/// once by `flash::SecurityFlash::new` (or the `flash::security_store`
/// helper, only compiled for `target_arch = "tc32"`).
pub struct SecurityPartition(());

/// Split a board's single onboard-flash ownership token into the disjoint
/// partition tokens this workspace's products use.
///
/// `token` is consumed by value. Combined with [`ChildTablePartition`],
/// [`AppNvPartition`], and [`SecurityPartition`] not implementing
/// `Clone`/`Copy`, this makes it
/// impossible for safe code to derive two independent accessors over the
/// same physical region: splitting the *same* onboard-flash token twice
/// does not type-check because the first call already moved it.
pub fn split_onboard_flash<T: OnboardFlashToken>(
    _token: T,
) -> (ChildTablePartition, AppNvPartition, SecurityPartition) {
    (
        ChildTablePartition(()),
        AppNvPartition(()),
        SecurityPartition(()),
    )
}

/// Compute the physical flash address for a `length`-byte access at
/// `offset` within a `region_size`-byte partition starting at
/// `region_start`, or `None` if the access would leave the partition or
/// overflow an address.
///
/// This is the shared bounds check behind `flash::ChildTableFlash`,
/// `flash::AppNvFlash`, and `flash::SecurityFlash`; it is plain,
/// `const`-friendly arithmetic with
/// no hardware dependency, so it is exercised directly by this crate's host
/// tests even though the `NorFlash` wrappers themselves only build for
/// `target_arch = "tc32"`.
pub const fn checked_partition_offset(
    region_start: u32,
    region_size: usize,
    offset: u32,
    length: usize,
) -> Option<u32> {
    let fits = match (offset as usize).checked_add(length) {
        Some(end) => end <= region_size,
        None => false,
    };
    if !fits {
        return None;
    }
    region_start.checked_add(offset)
}

/// The disabled, unproven candidate energy-journal region on 1 MiB TLSR8258
/// parts (`0x96000..0xFC000`).
///
/// This type only documents that the region is known; it grants no read,
/// write, or erase access. No product in this workspace may treat it as
/// available flash until it has its own verified staging, identity, and
/// activation policy (see the OTA design constraints in the repository's
/// architecture notes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateEnergyJournal {
    pub region: FlashRegion,
}

impl CandidateEnergyJournal {
    /// Return the disabled candidate region for `layout`, if the layout
    /// declares one. `TLSR8258_512K_LAYOUT` does not; `TLSR8258_1M_LAYOUT`
    /// does.
    pub const fn from_layout(layout: FlashLayout) -> Option<Self> {
        match layout.candidate_energy_journal {
            Some(region) => Some(Self { region }),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    // This crate is `#![no_std]` for the tc32 firmware build, but its host
    // test binary always links `std` anyway (the test harness requires
    // it); declaring it explicitly here just lets these tests use
    // `std::format!` for the linker-script cross-check below.
    extern crate std;

    use super::*;
    use core::mem::size_of;
    use std::string::String;
    use zigbee_plug_hardware::{TLSR8258_1M_LAYOUT, TLSR8258_512K_LAYOUT};

    /// A host-only stand-in for a genuine board token, used solely to
    /// exercise the sealed-trait dispatch in this crate's own tests. It
    /// deliberately lives in `tests` rather than at module scope so
    /// production code can never construct partition tokens from it.
    struct TestToken;
    impl sealed::Sealed for TestToken {}
    impl OnboardFlashToken for TestToken {}

    #[test]
    fn partition_tokens_are_zero_sized() {
        assert_eq!(size_of::<ChildTablePartition>(), 0);
        assert_eq!(size_of::<AppNvPartition>(), 0);
        assert_eq!(size_of::<SecurityPartition>(), 0);
    }

    #[test]
    fn split_onboard_flash_yields_all_partition_tokens() {
        let (_children, _app_nv, _security) = split_onboard_flash(TestToken);
    }

    #[test]
    fn bounds_check_accepts_in_range_access_and_rejects_overflow() {
        // Child-table journal: 0x2000 bytes starting at 0x70000.
        assert_eq!(
            checked_partition_offset(0x7_0000, 0x2000, 0, 512),
            Some(0x7_0000)
        );
        assert_eq!(
            checked_partition_offset(0x7_0000, 0x2000, 0x1E00, 512),
            Some(0x7_1E00)
        );
        assert_eq!(
            checked_partition_offset(0x7_0000, 0x2000, 0x1E00, 513),
            None
        );
        // Security journal: 0x2000 bytes starting at 0x74000.
        assert_eq!(
            checked_partition_offset(0x7_4000, 0x2000, 0, 128),
            Some(0x7_4000)
        );
        assert_eq!(
            checked_partition_offset(0x7_4000, 0x2000, 0x1F80, 128),
            Some(0x7_5F80)
        );
        // One byte past the partition end must be rejected.
        assert_eq!(
            checked_partition_offset(0x7_4000, 0x2000, 0x1F80, 129),
            None
        );
        assert_eq!(checked_partition_offset(0x7_4000, 0x2000, 0x2000, 1), None);
        // An address computation that overflows u32 must also be rejected.
        assert_eq!(checked_partition_offset(u32::MAX, 4096, 1, 1), None);
    }

    #[test]
    fn child_application_and_security_partitions_are_adjacent_and_disjoint() {
        for layout in [TLSR8258_512K_LAYOUT, TLSR8258_1M_LAYOUT] {
            assert!(layout.validate().is_ok());
            assert_eq!(layout.child_table_journal.end, layout.application_nv.start);
            assert_eq!(layout.application_nv.end, layout.security_journal.start);
            assert!(layout.firmware.end <= layout.child_table_journal.start);
        }
    }

    #[test]
    fn candidate_energy_journal_is_disabled_by_default_and_only_on_1m_parts() {
        assert_eq!(
            CandidateEnergyJournal::from_layout(TLSR8258_512K_LAYOUT),
            None
        );
        let candidate = CandidateEnergyJournal::from_layout(TLSR8258_1M_LAYOUT)
            .expect("1 MiB layout documents a candidate energy region");
        assert_eq!(candidate.region.start, 0x0009_6000);
        assert_eq!(candidate.region.end, 0x000F_C000);
    }

    /// Assert that `needle` (an assignment like `_app_nv_start_ = 0x72000;`)
    /// appears verbatim in `script`. Kept deliberately dumb (substring, not
    /// a linker-script parser) so it fails loudly and specifically the
    /// moment either side drifts, without pulling in a parsing dependency.
    fn assert_symbol_assignment(script: &str, symbol: &str, value: u32) {
        let needle = alloc_symbol_line(symbol, value);
        assert!(
            script.to_lowercase().contains(&needle.to_lowercase()),
            "expected canonical linker script to contain `{needle}` (case-insensitive)"
        );
    }

    fn alloc_symbol_line(symbol: &str, value: u32) -> String {
        std::format!("{symbol} = {value:#x};")
    }

    /// Ties the literal addresses in both canonical linker scripts back to
    /// `zigbee_plug_hardware`'s `FlashLayout` constants, so the two
    /// representations of the same partition catalog cannot silently drift
    /// apart. This is a text-substring check, not a linker-script parser —
    /// it only guards against accidental edits, not a from-scratch layout
    /// redesign.
    #[test]
    fn linker_scripts_match_flash_layout_catalog() {
        let script_512k = include_str!("../../link/tlsr8258-512k.x");
        let layout = TLSR8258_512K_LAYOUT;
        assert_symbol_assignment(
            script_512k,
            "_child_nv_start_",
            layout.child_table_journal.start,
        );
        assert_symbol_assignment(
            script_512k,
            "_child_nv_end_",
            layout.child_table_journal.end,
        );
        assert_symbol_assignment(script_512k, "_app_nv_start_", layout.application_nv.start);
        assert_symbol_assignment(script_512k, "_app_nv_end_", layout.application_nv.end);
        assert_symbol_assignment(
            script_512k,
            "_security_nv_start_",
            layout.security_journal.start,
        );
        assert_symbol_assignment(
            script_512k,
            "_security_nv_end_",
            layout.security_journal.end,
        );
        assert_symbol_assignment(
            script_512k,
            "_factory_data_start_",
            layout.factory_data.start,
        );
        assert_symbol_assignment(script_512k, "_factory_data_end_", layout.factory_data.end);
        assert_symbol_assignment(script_512k, "_flash_capacity_", layout.capacity);
        assert!(script_512k.split("_candidate_energy_start_").count() == 1);
        assert!(script_512k.contains("ASSERT(_bin_size_ < _child_nv_start_"));

        let script_1m = include_str!("../../link/tlsr8258-1m.x");
        let layout = TLSR8258_1M_LAYOUT;
        assert_symbol_assignment(
            script_1m,
            "_child_nv_start_",
            layout.child_table_journal.start,
        );
        assert_symbol_assignment(script_1m, "_child_nv_end_", layout.child_table_journal.end);
        assert_symbol_assignment(script_1m, "_app_nv_start_", layout.application_nv.start);
        assert_symbol_assignment(script_1m, "_app_nv_end_", layout.application_nv.end);
        assert_symbol_assignment(
            script_1m,
            "_security_nv_start_",
            layout.security_journal.start,
        );
        assert_symbol_assignment(script_1m, "_security_nv_end_", layout.security_journal.end);
        assert_symbol_assignment(script_1m, "_factory_data_start_", layout.factory_data.start);
        assert_symbol_assignment(script_1m, "_factory_data_end_", layout.factory_data.end);
        assert_symbol_assignment(script_1m, "_flash_capacity_", layout.capacity);
        let candidate = layout
            .candidate_energy_journal
            .expect("1 MiB layout documents a candidate energy region");
        assert_symbol_assignment(script_1m, "_candidate_energy_start_", candidate.start);
        assert_symbol_assignment(script_1m, "_candidate_energy_end_", candidate.end);
        assert!(script_1m.contains("ASSERT(_bin_size_ < _child_nv_start_"));
    }

    /// The build/check helper is documentation as much as it is a script:
    /// this pins its documented command/layout vocabulary so the usage
    /// text cannot silently drift from what the `case` statement actually
    /// accepts, without needing to execute the script (which requires a
    /// TC32 toolchain this host does not have).
    #[test]
    fn firmware_script_documents_its_commands_and_layouts() {
        let script = include_str!("../../scripts/tlsr8258-firmware.sh");
        for documented in ["check", "build", "512k", "1m"] {
            assert!(
                script.contains(documented),
                "expected scripts/tlsr8258-firmware.sh to document `{documented}`"
            );
        }
        assert!(
            !script.contains("\"flash\")") && !script.contains("'flash')"),
            "scripts/tlsr8258-firmware.sh must not implement a device-flashing command"
        );
        assert!(
            script.contains("TLSR8258_LINKER_SCRIPT"),
            "the helper must pass the canonical linker script through build.rs"
        );
        assert!(
            !script.contains("cp \"$link_script\" \"${crate_dir}/memory.x\""),
            "the helper must not generate memory.x inside the source tree"
        );
        assert!(
            script.to_lowercase().contains("experimental"),
            "scripts/tlsr8258-firmware.sh must label its output experimental"
        );
        assert!(
            script.contains("if (( size >= child_nv_start )); then"),
            "the helper must reject an image ending at the child partition boundary"
        );
    }
}
