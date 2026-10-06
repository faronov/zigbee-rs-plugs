//! `target_arch = "tc32"`-only `NorFlash` wrappers and persistence wiring.
//!
//! This module is compiled only for the real TLSR8258 target because it
//! wraps the bounded `tlsr8258_hal::flash::FlashRegion`, which itself only
//! exists for `target_arch = "tc32"` (it drives real MSPI registers).
//! Everything here is therefore validated by `cargo build`/`cargo check` for
//! the `tc32` target only — it has not been exercised on TLSR8258 hardware
//! from this crate. The host-testable model of the same partition bounds
//! rule lives in [`crate::checked_partition_offset`].

use core::marker::PhantomData;

use embedded_storage::nor_flash::{ErrorType, NorFlash, ReadNorFlash};
use tlsr8258_hal::flash::{FlashError, FlashRegion};
use zigbee_plug_hardware::FlashLayout;
use zigbee_runtime::child_store::{CHILD_JOURNAL_SECTOR_SIZE, ChildTableJournal};
use zigbee_runtime::log_nv::LogStructuredNv;
use zigbee_runtime::security_journal::{SECURITY_JOURNAL_SECTOR_SIZE, SecurityStateJournal};

// Re-exported so downstream product crates can wire persistence without
// taking a direct `zigbee-runtime` dependency of their own.
pub use zigbee_runtime::nv_storage::NvError;

use crate::{AppNvPartition, ChildTablePartition, SecurityPartition};

/// Uninhabited marker distinguishing [`AppNvFlash`] from [`SecurityFlash`]
/// at the type level, so the two partitions cannot be confused even though
/// they share one generic implementation.
pub struct AppNvRegion(());
/// Uninhabited marker distinguishing the child-table journal partition.
pub struct ChildTableRegion(());
/// See [`AppNvRegion`].
pub struct SecurityRegion(());

/// A single bounded, type-safe TLSR8258 flash partition.
///
/// `Region` is a zero-sized marker ([`AppNvRegion`] or [`SecurityRegion`])
/// that only distinguishes the two type aliases below; it carries no data.
/// The storage itself is the HAL's sector-aligned [`FlashRegion`], which
/// rejects every access outside the partition before a flash command is
/// issued, so persistence code never holds a whole-chip flash handle.
pub struct PartitionFlash<Region> {
    flash: FlashRegion,
    _region: PhantomData<Region>,
}

impl<Region> PartitionFlash<Region> {
    /// Build the bounded window `region` of a layout that passed
    /// [`FlashLayout::validate`].
    ///
    /// Callers reach this only through a consumed, single-use partition
    /// token, which is what makes the window exclusively owned.
    #[allow(unsafe_code)]
    const fn bounded(layout: FlashLayout, region: zigbee_plug_hardware::FlashRegion) -> Self {
        assert!(layout.validate().is_ok());
        // SAFETY: `layout.validate()` (asserted above, and const-asserted by
        // every product crate) proves the window lies inside the fitted
        // flash, after the firmware image and before the factory/calibration
        // sectors, and is disjoint from every other product partition. The
        // window is only built from a partition token that `split_flash`
        // mints exactly once, so no second handle covers these sectors.
        let flash = unsafe { FlashRegion::new(region.start, region.size() as usize) };
        Self {
            flash,
            _region: PhantomData,
        }
    }
}

impl<Region> ErrorType for PartitionFlash<Region> {
    type Error = FlashError;
}

impl<Region> ReadNorFlash for PartitionFlash<Region> {
    const READ_SIZE: usize = FlashRegion::READ_SIZE;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        self.flash.read(offset, bytes)
    }

    fn capacity(&self) -> usize {
        self.flash.size()
    }
}

impl<Region> NorFlash for PartitionFlash<Region> {
    const WRITE_SIZE: usize = FlashRegion::WRITE_SIZE;
    const ERASE_SIZE: usize = FlashRegion::ERASE_SIZE;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        self.flash.erase(from, to)
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        self.flash.write(offset, bytes)
    }
}

/// The product-owned application-NV log partition (`0x72000..0x74000`).
pub type AppNvFlash = PartitionFlash<AppNvRegion>;
/// The durable Zigbee child-table journal partition (`0x70000..0x72000`).
pub type ChildTableFlash = PartitionFlash<ChildTableRegion>;
/// The Zigbee security-counter journal partition (`0x74000..0x76000`).
pub type SecurityFlash = PartitionFlash<SecurityRegion>;

impl AppNvFlash {
    /// Consume the [`AppNvPartition`] token to construct the bounded
    /// application-NV flash accessor described by `layout`.
    pub const fn new(_token: AppNvPartition, layout: FlashLayout) -> Self {
        Self::bounded(layout, layout.application_nv)
    }
}

impl ChildTableFlash {
    /// Consume the [`ChildTablePartition`] token to construct the bounded
    /// child-table journal flash accessor described by `layout`.
    pub const fn new(_token: ChildTablePartition, layout: FlashLayout) -> Self {
        Self::bounded(layout, layout.child_table_journal)
    }
}

impl SecurityFlash {
    /// Consume the [`SecurityPartition`] token to construct the bounded
    /// security-journal flash accessor described by `layout`.
    pub const fn new(_token: SecurityPartition, layout: FlashLayout) -> Self {
        Self::bounded(layout, layout.security_journal)
    }
}

/// Product-owned application-NV log store.
pub type ApplicationNv = LogStructuredNv<AppNvFlash>;
/// Product-owned durable Zigbee child-table journal.
pub type ChildStore = ChildTableJournal<ChildTableFlash>;
/// Product-owned Zigbee security-counter journal store.
pub type SecurityStore = SecurityStateJournal<SecurityFlash>;

const NV_PAGE_A: u32 = 0;
const NV_PAGE_B: u32 = SECURITY_JOURNAL_SECTOR_SIZE as u32;
const CHILD_SECTOR_A: u32 = 0;
const CHILD_SECTOR_B: u32 = CHILD_JOURNAL_SECTOR_SIZE as u32;
const SECURITY_SECTOR_A: u32 = 0;
const SECURITY_SECTOR_B: u32 = SECURITY_JOURNAL_SECTOR_SIZE as u32;

/// Build the two-sector application-NV log over `layout`'s application-NV
/// partition, consuming `token` exactly once.
///
/// `layout.application_nv` must be exactly two `Tlsr8258Flash::ERASE_SIZE`
/// sectors, matching every layout in
/// `zigbee_plug_hardware::{TLSR8258_512K_LAYOUT, TLSR8258_1M_LAYOUT}`.
pub fn application_nv(
    token: AppNvPartition,
    layout: FlashLayout,
) -> Result<ApplicationNv, NvError> {
    LogStructuredNv::new(AppNvFlash::new(token, layout), NV_PAGE_A, NV_PAGE_B)
}

/// Build the durable child-table journal over `layout`'s dedicated
/// two-sector partition, consuming `token` exactly once.
pub const fn child_table_store(token: ChildTablePartition, layout: FlashLayout) -> ChildStore {
    ChildTableJournal::new(
        ChildTableFlash::new(token, layout),
        CHILD_SECTOR_A,
        CHILD_SECTOR_B,
    )
}

/// Build the Zigbee security-counter journal over `layout`'s security
/// partition, consuming `token` exactly once.
pub fn security_store(token: SecurityPartition, layout: FlashLayout) -> SecurityStore {
    SecurityStateJournal::new(
        SecurityFlash::new(token, layout),
        SECURITY_SECTOR_A,
        SECURITY_SECTOR_B,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use zigbee_plug_hardware::{TLSR8258_1M_LAYOUT, TLSR8258_512K_LAYOUT};

    const _: () = assert!(
        TLSR8258_512K_LAYOUT.application_nv.size() as usize == 2 * Tlsr8258Flash::ERASE_SIZE
    );
    const _: () =
        assert!(TLSR8258_1M_LAYOUT.application_nv.size() as usize == 2 * Tlsr8258Flash::ERASE_SIZE);
    const _: () = assert!(
        TLSR8258_512K_LAYOUT.child_table_journal.size() as usize == 2 * CHILD_JOURNAL_SECTOR_SIZE
    );
    const _: () = assert!(
        TLSR8258_1M_LAYOUT.child_table_journal.size() as usize == 2 * CHILD_JOURNAL_SECTOR_SIZE
    );
    const _: () = assert!(
        TLSR8258_512K_LAYOUT.security_journal.size() as usize == 2 * SECURITY_JOURNAL_SECTOR_SIZE
    );
    const _: () = assert!(
        TLSR8258_1M_LAYOUT.security_journal.size() as usize == 2 * SECURITY_JOURNAL_SECTOR_SIZE
    );
}
