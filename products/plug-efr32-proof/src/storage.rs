//! Product-owned crash-safe application and security journals.
//!
//! The End Device role intentionally has no child-table store. Application
//! state owns `0x78000..0x7BFFF`; security owns `0x7C000..0x7FFFF`.

use core::{cell::RefCell, marker::PhantomData};

use efr32mg21_brd4181a_plug::storage::StorageToken;
use efr32mg21_hal::flash::{Efr32mg21Flash, FLASH_CAPACITY, FLASH_PAGE_SIZE, FlashError};
use embedded_storage::nor_flash::{ErrorType, NorFlash, ReadNorFlash};
use static_cell::StaticCell;
use zigbee_runtime::{
    nv_storage::{NvError, NvItemId, NvStorage},
    security_journal::SecurityStateJournal,
};

pub const APPLICATION_FLASH_START: u32 = 0x0007_8000;
pub const APPLICATION_FLASH_SIZE: usize = 16 * 1024;
pub const APPLICATION_FLASH_END: u32 = APPLICATION_FLASH_START + APPLICATION_FLASH_SIZE as u32;
pub const SECURITY_FLASH_START: u32 = 0x0007_C000;
pub const SECURITY_FLASH_SIZE: usize = 16 * 1024;
pub const SECURITY_FLASH_END: u32 = SECURITY_FLASH_START + SECURITY_FLASH_SIZE as u32;

const APP_SECTOR_A: u32 = 0;
const APP_SECTOR_B: u32 = FLASH_PAGE_SIZE as u32;
const SECURITY_SECTOR_SIZE: usize = FLASH_PAGE_SIZE;
const SECURITY_SECTOR_A: u32 = 0;
const SECURITY_SECTOR_B: u32 = SECURITY_SECTOR_SIZE as u32;

const _: () = assert!(APPLICATION_FLASH_START == 0x0007_8000);
const _: () = assert!(APPLICATION_FLASH_SIZE == 2 * FLASH_PAGE_SIZE);
const _: () = assert!(APPLICATION_FLASH_END == SECURITY_FLASH_START);
const _: () = assert!(SECURITY_FLASH_SIZE == 2 * SECURITY_SECTOR_SIZE);
const _: () = assert!(SECURITY_SECTOR_SIZE == FLASH_PAGE_SIZE);
const _: () = assert!(SECURITY_FLASH_END as usize == FLASH_CAPACITY);

/// Caller-owned one-time slot for the physical flash-controller token.
pub struct FlashStorageCell {
    inner: StaticCell<RefCell<Efr32mg21Flash>>,
}

impl FlashStorageCell {
    pub const fn new() -> Self {
        Self {
            inner: StaticCell::new(),
        }
    }
}

impl Default for FlashStorageCell {
    fn default() -> Self {
        Self::new()
    }
}

pub enum ApplicationRegion {}
pub enum SecurityRegion {}

pub type ApplicationFlash = PartitionFlash<ApplicationRegion>;
pub type SecurityFlash = PartitionFlash<SecurityRegion>;

/// Product-owned, type-distinguished bounds-checked view of internal flash.
pub struct PartitionFlash<Region> {
    flash: &'static RefCell<Efr32mg21Flash>,
    start: u32,
    size: usize,
    _region: PhantomData<Region>,
}

impl<Region> PartitionFlash<Region> {
    const fn new(flash: &'static RefCell<Efr32mg21Flash>, start: u32, size: usize) -> Self {
        Self {
            flash,
            start,
            size,
            _region: PhantomData,
        }
    }

    fn physical_offset(&self, offset: u32, length: usize) -> Result<u32, FlashError> {
        (offset as usize)
            .checked_add(length)
            .filter(|end| *end <= self.size)
            .ok_or(FlashError::OutOfBounds)?;
        self.start
            .checked_add(offset)
            .ok_or(FlashError::OutOfBounds)
    }
}

impl<Region> ErrorType for PartitionFlash<Region> {
    type Error = FlashError;
}

impl<Region> ReadNorFlash for PartitionFlash<Region> {
    const READ_SIZE: usize = Efr32mg21Flash::READ_SIZE;

    fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
        let physical = self.physical_offset(offset, bytes.len())?;
        self.flash
            .try_borrow_mut()
            .map_err(|_| FlashError::Controller)?
            .read(physical, bytes)
    }

    fn capacity(&self) -> usize {
        self.size
    }
}

impl<Region> NorFlash for PartitionFlash<Region> {
    const WRITE_SIZE: usize = Efr32mg21Flash::WRITE_SIZE;
    const ERASE_SIZE: usize = Efr32mg21Flash::ERASE_SIZE;

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        if from >= to {
            return Err(FlashError::OutOfBounds);
        }
        let length = usize::try_from(to - from).map_err(|_| FlashError::OutOfBounds)?;
        let physical_from = self.physical_offset(from, length)?;
        let physical_to = physical_from
            .checked_add(to - from)
            .ok_or(FlashError::OutOfBounds)?;
        self.flash
            .try_borrow_mut()
            .map_err(|_| FlashError::Controller)?
            .erase(physical_from, physical_to)
    }

    fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
        let physical = self.physical_offset(offset, bytes.len())?;
        self.flash
            .try_borrow_mut()
            .map_err(|_| FlashError::Controller)?
            .write(physical, bytes)
    }
}

pub type ApplicationStore = AppStateJournal<ApplicationFlash>;
pub type SecurityStore = SecurityStateJournal<SecurityFlash, SECURITY_SECTOR_SIZE>;

pub fn open_storage(
    token: StorageToken,
    cell: &'static FlashStorageCell,
) -> Result<(ApplicationStore, SecurityStore), NvError> {
    let shared = cell.inner.init(RefCell::new(token.into_flash()));
    let application = PartitionFlash::new(shared, APPLICATION_FLASH_START, APPLICATION_FLASH_SIZE);
    let security = PartitionFlash::new(shared, SECURITY_FLASH_START, SECURITY_FLASH_SIZE);
    let application = AppStateJournal::new(application, APP_SECTOR_A, APP_SECTOR_B)?;
    let security =
        SecurityStore::new_with_sector_size(security, SECURITY_SECTOR_A, SECURITY_SECTOR_B);
    Ok((application, security))
}

const SLOT_SIZE: usize = 64;
const DATA_CAPACITY: usize = 32;
const MAGIC: [u8; 4] = *b"ZBPA";
const VERSION: u8 = 1;
const DATA_OFFSET: usize = 12;
const CRC_OFFSET: usize = 44;
const PREFIX_LEN: usize = 48;
const COMMIT_OFFSET: usize = 60;
const COMMIT: [u8; 4] = *b"CMIT";

const _: () = assert!(DATA_OFFSET + DATA_CAPACITY == CRC_OFFSET);
const _: () = assert!(CRC_OFFSET + 4 == PREFIX_LEN);
const _: () = assert!(COMMIT_OFFSET + COMMIT.len() == SLOT_SIZE);
const _: () = assert!(FLASH_PAGE_SIZE.is_multiple_of(SLOT_SIZE));

#[derive(Clone, Copy)]
struct LocatedRecord {
    generation: u32,
    sector: usize,
    length: usize,
    data: [u8; DATA_CAPACITY],
}

/// Two-sector, commit-last journal implementing the one application item used
/// by `plug-router-app`.
pub struct AppStateJournal<F: NorFlash> {
    flash: F,
    sectors: [u32; 2],
    cached: Option<LocatedRecord>,
    scanned: bool,
}

impl<F: NorFlash> AppStateJournal<F> {
    pub fn new(flash: F, first_sector: u32, second_sector: u32) -> Result<Self, NvError> {
        let journal = Self {
            flash,
            sectors: [first_sector, second_sector],
            cached: None,
            scanned: false,
        };
        if !journal.geometry_is_valid() {
            return Err(NvError::HardwareError);
        }
        Ok(journal)
    }

    pub fn flash(&self) -> &F {
        &self.flash
    }

    pub fn into_flash(self) -> F {
        self.flash
    }

    fn geometry_is_valid(&self) -> bool {
        self.sectors[0] != self.sectors[1]
            && self.sectors[0].abs_diff(self.sectors[1]) >= F::ERASE_SIZE as u32
            && F::READ_SIZE != 0
            && F::WRITE_SIZE != 0
            && F::ERASE_SIZE == FLASH_PAGE_SIZE
            && SLOT_SIZE.is_multiple_of(F::READ_SIZE)
            && SLOT_SIZE.is_multiple_of(F::WRITE_SIZE)
            && PREFIX_LEN.is_multiple_of(F::WRITE_SIZE)
            && COMMIT_OFFSET.is_multiple_of(F::WRITE_SIZE)
            && COMMIT.len().is_multiple_of(F::WRITE_SIZE)
            && self
                .sectors
                .iter()
                .all(|sector| (*sector as usize).is_multiple_of(F::ERASE_SIZE))
            && self.sectors.iter().all(|sector| {
                (*sector as usize)
                    .checked_add(F::ERASE_SIZE)
                    .is_some_and(|end| end <= self.flash.capacity())
            })
    }

    fn slots_per_sector() -> usize {
        F::ERASE_SIZE / SLOT_SIZE
    }

    fn read_slot(
        &mut self,
        sector: usize,
        slot: usize,
        output: &mut [u8; SLOT_SIZE],
    ) -> Result<(), NvError> {
        self.flash
            .read(self.sectors[sector] + (slot * SLOT_SIZE) as u32, output)
            .map_err(|_| NvError::HardwareError)
    }

    fn decode(record: &[u8; SLOT_SIZE]) -> Option<(u32, usize, [u8; DATA_CAPACITY])> {
        if record[0..4] != MAGIC
            || record[4] != VERSION
            || record[6..8] != (NvItemId::AppEndpoint1 as u16).to_le_bytes()
            || record[COMMIT_OFFSET..COMMIT_OFFSET + COMMIT.len()] != COMMIT
        {
            return None;
        }
        let length = record[5] as usize;
        if length > DATA_CAPACITY {
            return None;
        }
        let expected_crc = u32::from_le_bytes([
            record[CRC_OFFSET],
            record[CRC_OFFSET + 1],
            record[CRC_OFFSET + 2],
            record[CRC_OFFSET + 3],
        ]);
        if crc32(&record[..CRC_OFFSET]) != expected_crc {
            return None;
        }

        let generation = u32::from_le_bytes([record[8], record[9], record[10], record[11]]);
        let mut data = [0u8; DATA_CAPACITY];
        data.copy_from_slice(&record[DATA_OFFSET..DATA_OFFSET + DATA_CAPACITY]);
        Some((generation, length, data))
    }

    fn is_newer(candidate: u32, current: u32) -> bool {
        candidate != current && candidate.wrapping_sub(current) < 0x8000_0000
    }

    fn newest(&mut self) -> Result<Option<LocatedRecord>, NvError> {
        let mut newest: Option<LocatedRecord> = None;
        let mut record = [0u8; SLOT_SIZE];
        for sector in 0..2 {
            for slot in 0..Self::slots_per_sector() {
                self.read_slot(sector, slot, &mut record)?;
                let Some((generation, length, data)) = Self::decode(&record) else {
                    continue;
                };
                if newest.is_none_or(|current| Self::is_newer(generation, current.generation)) {
                    newest = Some(LocatedRecord {
                        generation,
                        sector,
                        length,
                        data,
                    });
                }
            }
        }
        Ok(newest)
    }

    fn current(&mut self) -> Result<Option<LocatedRecord>, NvError> {
        if !self.scanned {
            self.cached = self.newest()?;
            self.scanned = true;
        }
        Ok(self.cached)
    }

    fn first_erased_slot(&mut self, sector: usize) -> Result<Option<usize>, NvError> {
        let mut record = [0u8; SLOT_SIZE];
        for slot in 0..Self::slots_per_sector() {
            self.read_slot(sector, slot, &mut record)?;
            if record.iter().all(|byte| *byte == 0xFF) {
                return Ok(Some(slot));
            }
        }
        Ok(None)
    }

    fn write_record(
        &mut self,
        sector: usize,
        slot: usize,
        generation: u32,
        data: &[u8],
    ) -> Result<LocatedRecord, NvError> {
        if data.len() > DATA_CAPACITY {
            return Err(NvError::BufferTooSmall);
        }

        let mut record = [0xFFu8; SLOT_SIZE];
        record[0..4].copy_from_slice(&MAGIC);
        record[4] = VERSION;
        record[5] = data.len() as u8;
        record[6..8].copy_from_slice(&(NvItemId::AppEndpoint1 as u16).to_le_bytes());
        record[8..12].copy_from_slice(&generation.to_le_bytes());
        record[DATA_OFFSET..DATA_OFFSET + data.len()].copy_from_slice(data);
        let crc = crc32(&record[..CRC_OFFSET]);
        record[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());

        let address = self.sectors[sector] + (slot * SLOT_SIZE) as u32;
        self.flash
            .write(address, &record[..PREFIX_LEN])
            .map_err(|_| NvError::HardwareError)?;
        self.flash
            .write(address + COMMIT_OFFSET as u32, &COMMIT)
            .map_err(|_| NvError::HardwareError)?;

        let mut verify = [0u8; SLOT_SIZE];
        self.read_slot(sector, slot, &mut verify)?;
        let Some((stored_generation, stored_length, stored_data)) = Self::decode(&verify) else {
            return Err(NvError::HardwareError);
        };
        if stored_generation != generation
            || stored_length != data.len()
            || stored_data[..stored_length] != *data
        {
            return Err(NvError::HardwareError);
        }
        Ok(LocatedRecord {
            generation,
            sector,
            length: stored_length,
            data: stored_data,
        })
    }

    fn append(&mut self, data: &[u8]) -> Result<(), NvError> {
        let current = self.current()?;
        let generation = current.map_or(Ok(0), |record| {
            record.generation.checked_add(1).ok_or(NvError::Full)
        })?;

        let result = if let Some(record) = current {
            if let Some(slot) = self.first_erased_slot(record.sector)? {
                self.write_record(record.sector, slot, generation, data)
            } else {
                let target = 1 - record.sector;
                let address = self.sectors[target];
                self.flash
                    .erase(address, address + F::ERASE_SIZE as u32)
                    .map_err(|_| NvError::HardwareError)
                    .and_then(|()| self.write_record(target, 0, generation, data))
            }
        } else {
            let target = 0;
            if let Some(slot) = self.first_erased_slot(target)? {
                self.write_record(target, slot, generation, data)
            } else {
                let address = self.sectors[target];
                self.flash
                    .erase(address, address + F::ERASE_SIZE as u32)
                    .map_err(|_| NvError::HardwareError)
                    .and_then(|()| self.write_record(target, 0, generation, data))
            }
        };

        match result {
            Ok(located) => {
                self.cached = Some(located);
                Ok(())
            }
            Err(error) => {
                self.cached = None;
                self.scanned = false;
                Err(error)
            }
        }
    }

    fn validate_id(id: NvItemId) -> Result<(), NvError> {
        if id == NvItemId::AppEndpoint1 {
            Ok(())
        } else {
            Err(NvError::HardwareError)
        }
    }
}

impl<F: NorFlash> NvStorage for AppStateJournal<F> {
    fn read(&mut self, id: NvItemId, output: &mut [u8]) -> Result<usize, NvError> {
        Self::validate_id(id)?;
        let record = self.current()?.ok_or(NvError::NotFound)?;
        if record.length == 0 {
            return Err(NvError::NotFound);
        }
        if output.len() < record.length {
            return Err(NvError::BufferTooSmall);
        }
        output[..record.length].copy_from_slice(&record.data[..record.length]);
        Ok(record.length)
    }

    fn write(&mut self, id: NvItemId, data: &[u8]) -> Result<(), NvError> {
        Self::validate_id(id)?;
        self.append(data)
    }

    fn delete(&mut self, id: NvItemId) -> Result<(), NvError> {
        Self::validate_id(id)?;
        if !self.exists(id)? {
            return Err(NvError::NotFound);
        }
        self.append(&[])
    }

    fn exists(&mut self, id: NvItemId) -> Result<bool, NvError> {
        Self::validate_id(id)?;
        Ok(self.current()?.is_some_and(|record| record.length != 0))
    }

    fn item_length(&mut self, id: NvItemId) -> Result<usize, NvError> {
        Self::validate_id(id)?;
        self.current()?
            .filter(|record| record.length != 0)
            .map(|record| record.length)
            .ok_or(NvError::NotFound)
    }

    fn compact(&mut self) -> Result<(), NvError> {
        let Some(current) = self.current()? else {
            return Ok(());
        };
        let generation = current.generation.checked_add(1).ok_or(NvError::Full)?;
        let target = 1 - current.sector;
        let address = self.sectors[target];
        let result = self
            .flash
            .erase(address, address + F::ERASE_SIZE as u32)
            .map_err(|_| NvError::HardwareError)
            .and_then(|()| {
                self.write_record(target, 0, generation, &current.data[..current.length])
            });
        match result {
            Ok(located) => {
                self.cached = Some(located);
                Ok(())
            }
            Err(error) => {
                self.cached = None;
                self.scanned = false;
                Err(error)
            }
        }
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use embedded_storage::nor_flash::{ErrorType, NorFlashErrorKind, ReadNorFlash};
    use zigbee_plug_core::PlugStateRecord;

    #[derive(Clone)]
    struct MockFlash {
        data: [u8; APPLICATION_FLASH_SIZE],
        operations_before_failure: Option<usize>,
    }

    impl MockFlash {
        fn new() -> Self {
            Self {
                data: [0xFF; APPLICATION_FLASH_SIZE],
                operations_before_failure: None,
            }
        }

        fn fail_after(&mut self, operations: usize) {
            self.operations_before_failure = Some(operations);
        }

        fn clear_failure(&mut self) {
            self.operations_before_failure = None;
        }

        fn should_fail(&mut self) -> bool {
            let Some(remaining) = self.operations_before_failure.as_mut() else {
                return false;
            };
            if *remaining == 0 {
                true
            } else {
                *remaining -= 1;
                false
            }
        }

        fn range(
            address: u32,
            length: usize,
        ) -> Result<core::ops::Range<usize>, NorFlashErrorKind> {
            let start = address as usize;
            let end = start
                .checked_add(length)
                .filter(|end| *end <= APPLICATION_FLASH_SIZE)
                .ok_or(NorFlashErrorKind::OutOfBounds)?;
            Ok(start..end)
        }
    }

    impl ErrorType for MockFlash {
        type Error = NorFlashErrorKind;
    }

    impl ReadNorFlash for MockFlash {
        const READ_SIZE: usize = 1;

        fn read(&mut self, address: u32, output: &mut [u8]) -> Result<(), Self::Error> {
            output.copy_from_slice(&self.data[Self::range(address, output.len())?]);
            Ok(())
        }

        fn capacity(&self) -> usize {
            self.data.len()
        }
    }

    impl NorFlash for MockFlash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = FLASH_PAGE_SIZE;

        fn write(&mut self, address: u32, input: &[u8]) -> Result<(), Self::Error> {
            if !(address as usize).is_multiple_of(Self::WRITE_SIZE)
                || !input.len().is_multiple_of(Self::WRITE_SIZE)
            {
                return Err(NorFlashErrorKind::NotAligned);
            }
            let range = Self::range(address, input.len())?;
            let (words, remainder) = input.as_chunks::<4>();
            debug_assert!(remainder.is_empty());
            for (word_index, word) in words.iter().enumerate() {
                let start = range.start + word_index * Self::WRITE_SIZE;
                if self.should_fail() {
                    for (old, new) in self.data[start..start + 2].iter_mut().zip(word) {
                        *old &= *new;
                    }
                    return Err(NorFlashErrorKind::Other);
                }
                for (old, new) in self.data[start..start + 4].iter_mut().zip(word) {
                    if (*old & *new) != *new {
                        return Err(NorFlashErrorKind::Other);
                    }
                    *old &= *new;
                }
            }
            Ok(())
        }

        fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            if from >= to
                || !(from as usize).is_multiple_of(Self::ERASE_SIZE)
                || !(to as usize).is_multiple_of(Self::ERASE_SIZE)
            {
                return Err(NorFlashErrorKind::NotAligned);
            }
            let range = Self::range(from, (to - from) as usize)?;
            if self.should_fail() {
                let midpoint = range.start + range.len() / 2;
                self.data[range.start..midpoint].fill(0xFF);
                return Err(NorFlashErrorKind::Other);
            }
            self.data[range].fill(0xFF);
            Ok(())
        }
    }

    fn journal(flash: MockFlash) -> AppStateJournal<MockFlash> {
        AppStateJournal::new(flash, APP_SECTOR_A, APP_SECTOR_B).unwrap()
    }

    fn record(sequence: u32) -> [u8; PlugStateRecord::LEN] {
        PlugStateRecord::new(sequence, sequence & 1 != 0, u64::from(sequence) * 1_000).encode()
    }

    fn read_sequence(mut flash: MockFlash) -> Option<u32> {
        flash.clear_failure();
        let mut store = journal(flash);
        let mut bytes = [0u8; PlugStateRecord::LEN];
        store
            .read(NvItemId::AppEndpoint1, &mut bytes)
            .ok()
            .and_then(|_| PlugStateRecord::decode(bytes).ok())
            .map(|record| record.sequence)
    }

    #[test]
    fn flash_map_matches_linker_and_has_no_child_partition() {
        assert_eq!(APPLICATION_FLASH_START, 0x0007_8000);
        assert_eq!(APPLICATION_FLASH_END, 0x0007_C000);
        assert_eq!(SECURITY_FLASH_START, 0x0007_C000);
        assert_eq!(SECURITY_FLASH_END, 0x0008_0000);

        let linker = include_str!("../link/memory.x");
        for literal in [
            "ORIGIN = 0x00078000",
            "ORIGIN = 0x0007C000",
            "_app_nv_start_",
            "_security_nv_start_",
        ] {
            assert!(linker.contains(literal), "missing linker literal {literal}");
        }
        assert!(!linker.to_ascii_lowercase().contains("child"));
    }

    #[test]
    fn app_state_round_trips_and_tombstones() {
        let mut store = journal(MockFlash::new());
        store.write(NvItemId::AppEndpoint1, &record(1)).unwrap();
        assert_eq!(read_sequence(store.flash.clone()), Some(1));
        store.delete(NvItemId::AppEndpoint1).unwrap();
        assert_eq!(read_sequence(store.into_flash()), None);
    }

    #[test]
    fn every_interrupted_append_preserves_previous_record() {
        let mut initial = journal(MockFlash::new());
        initial.write(NvItemId::AppEndpoint1, &record(1)).unwrap();
        let baseline = initial.into_flash();

        // 48-byte prefix (12 words), followed by one commit word.
        for cut in 0..13 {
            let mut flash = baseline.clone();
            flash.fail_after(cut);
            let mut interrupted = journal(flash);
            assert_eq!(
                interrupted.write(NvItemId::AppEndpoint1, &record(2)),
                Err(NvError::HardwareError),
                "cut {cut}"
            );
            assert_eq!(
                read_sequence(interrupted.into_flash()),
                Some(1),
                "cut {cut}"
            );
        }
    }

    #[test]
    fn every_interrupted_rollover_preserves_full_source_sector() {
        let mut full = journal(MockFlash::new());
        for sequence in 1..=AppStateJournal::<MockFlash>::slots_per_sector() as u32 {
            full.write(NvItemId::AppEndpoint1, &record(sequence))
                .unwrap();
        }
        let baseline = full.into_flash();
        let next = AppStateJournal::<MockFlash>::slots_per_sector() as u32 + 1;

        // One erase, twelve prefix words, and one commit word.
        for cut in 0..14 {
            let mut flash = baseline.clone();
            flash.fail_after(cut);
            let mut interrupted = journal(flash);
            assert_eq!(
                interrupted.write(NvItemId::AppEndpoint1, &record(next)),
                Err(NvError::HardwareError),
                "cut {cut}"
            );
            assert_eq!(
                read_sequence(interrupted.into_flash()),
                Some(next - 1),
                "cut {cut}"
            );
        }
    }

    #[test]
    fn crc_corruption_falls_back_to_previous_generation() {
        let mut store = journal(MockFlash::new());
        store.write(NvItemId::AppEndpoint1, &record(1)).unwrap();
        store.write(NvItemId::AppEndpoint1, &record(2)).unwrap();
        let mut flash = store.into_flash();
        flash.data[SLOT_SIZE + DATA_OFFSET] ^= 1;
        assert_eq!(read_sequence(flash), Some(1));
    }
}
