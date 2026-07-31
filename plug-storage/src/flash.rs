//! `target_arch = "tc32"`-only `NorFlash` wrappers and persistence wiring.
//!
//! This module is compiled only for the real TLSR8258 target because it
//! wraps `tlsr8258_hal::flash::Tlsr8258Flash`, which itself only exists for
//! `target_arch = "tc32"` (it drives real MSPI registers). Everything here
//! is therefore validated by `cargo build`/`cargo check` for the `tc32`
//! target only — it has not been exercised on TLSR8258 hardware from this
//! crate. Bounds arithmetic shared with the host-testable parts of this
//! crate lives in [`crate::checked_partition_offset`].

use core::marker::PhantomData;

use embedded_storage::nor_flash::{ErrorType, NorFlash, ReadNorFlash};
use tlsr8258_hal::flash::{FlashError, Tlsr8258Flash};
use zigbee_plug_hardware::FlashLayout;
use zigbee_runtime::log_nv::LogStructuredNv;
use zigbee_runtime::security_journal::{SECURITY_JOURNAL_SECTOR_SIZE, SecurityStateJournal};

// Re-exported so downstream product crates can wire persistence without
// taking a direct `zigbee-runtime` dependency of their own.
pub use zigbee_runtime::nv_storage::NvError;

use crate::{AppNvPartition, SecurityPartition, checked_partition_offset};

/// Uninhabited marker distinguishing [`AppNvFlash`] from [`SecurityFlash`]
/// at the type level, so the two partitions cannot be confused even though
/// they share one generic implementation.
pub struct AppNvRegion(());
/// See [`AppNvRegion`].
pub struct SecurityRegion(());

/// A single bounded, type-safe TLSR8258 flash partition.
///
/// `Region` is a zero-sized marker ([`AppNvRegion`] or [`SecurityRegion`])
/// that only distinguishes the two type aliases below; it carries no data.
pub struct PartitionFlash<Region> {
    flash: Tlsr8258Flash,
    start: u32,
    size: usize,
    _region: PhantomData<Region>,
}

impl<Region> PartitionFlash<Region> {
    fn physical_offset(&self, offset: u32, length: usize) -> Result<u32, FlashError> {
        checked_partition_offset(self.start, self.size, offset, length)
            .ok_or(FlashError::AddressOverflow)
    }
}

impl<Region> ErrorType for PartitionFlash<Region> {
    type Error = FlashError;
}

impl<Region> ReadNorFlash for PartitionFlash<Region> {
    const READ_SIZE: usize = Tlsr8258Flash::READ_SIZE;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let physical = self.physical_offset(offset, bytes.len())?;
        self.flash.read(physical, bytes)
    }

    fn capacity(&self) -> usize {
        self.size
    }
}

impl<Region> NorFlash for PartitionFlash<Region> {
    const WRITE_SIZE: usize = Tlsr8258Flash::WRITE_SIZE;
    const ERASE_SIZE: usize = Tlsr8258Flash::ERASE_SIZE;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        if from >= to {
            return Err(FlashError::AddressOverflow);
        }
        let length = usize::try_from(to - from).map_err(|_| FlashError::AddressOverflow)?;
        let physical_from = self.physical_offset(from, length)?;
        let physical_to = physical_from
            .checked_add(to - from)
            .ok_or(FlashError::AddressOverflow)?;
        self.flash.erase(physical_from, physical_to)
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        let physical = self.physical_offset(offset, bytes.len())?;
        self.flash.write(physical, bytes)
    }
}

/// The product-owned application-NV log partition (`0x72000..0x74000`).
pub type AppNvFlash = PartitionFlash<AppNvRegion>;
/// The Zigbee security-counter journal partition (`0x74000..0x76000`).
pub type SecurityFlash = PartitionFlash<SecurityRegion>;

impl AppNvFlash {
    /// Consume the [`AppNvPartition`] token to construct the bounded
    /// application-NV flash accessor described by `layout`.
    pub const fn new(_token: AppNvPartition, layout: FlashLayout) -> Self {
        Self {
            flash: Tlsr8258Flash::new(layout.capacity as usize),
            start: layout.application_nv.start,
            size: layout.application_nv.size() as usize,
            _region: PhantomData,
        }
    }
}

impl SecurityFlash {
    /// Consume the [`SecurityPartition`] token to construct the bounded
    /// security-journal flash accessor described by `layout`.
    pub const fn new(_token: SecurityPartition, layout: FlashLayout) -> Self {
        Self {
            flash: Tlsr8258Flash::new(layout.capacity as usize),
            start: layout.security_journal.start,
            size: layout.security_journal.size() as usize,
            _region: PhantomData,
        }
    }
}

/// Product-owned application-NV log store.
pub type ApplicationNv = LogStructuredNv<AppNvFlash>;
/// Product-owned Zigbee security-counter journal store.
pub type SecurityStore = SecurityStateJournal<SecurityFlash>;

const NV_PAGE_A: u32 = 0;
const NV_PAGE_B: u32 = SECURITY_JOURNAL_SECTOR_SIZE as u32;
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
        TLSR8258_512K_LAYOUT.security_journal.size() as usize == 2 * SECURITY_JOURNAL_SECTOR_SIZE
    );
    const _: () = assert!(
        TLSR8258_1M_LAYOUT.security_journal.size() as usize == 2 * SECURITY_JOURNAL_SECTOR_SIZE
    );
}
