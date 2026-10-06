use core::ops::Range;

use embedded_storage::nor_flash::{ErrorType, NorFlash, NorFlashErrorKind, ReadNorFlash};
use zigbee_runtime::child_store::{
    CHILD_JOURNAL_SECTOR_SIZE, CHILD_JOURNAL_SLOT_SIZE, ChildStoreError, ChildTableJournal,
    ChildTableStore, PersistentChild, PersistentChildTable,
};

const EXTENDED_PAN_ID: [u8; 8] = [0xA5; 8];
const FLASH_SIZE: usize = CHILD_JOURNAL_SECTOR_SIZE * 2;

struct TestFlash {
    bytes: [u8; FLASH_SIZE],
    writes_before_failure: Option<usize>,
}

impl TestFlash {
    fn erased() -> Self {
        Self {
            bytes: [0xFF; FLASH_SIZE],
            writes_before_failure: None,
        }
    }

    fn range(address: u32, length: usize) -> Result<Range<usize>, NorFlashErrorKind> {
        let start = usize::try_from(address).map_err(|_| NorFlashErrorKind::OutOfBounds)?;
        let end = start
            .checked_add(length)
            .filter(|end| *end <= FLASH_SIZE)
            .ok_or(NorFlashErrorKind::OutOfBounds)?;
        Ok(start..end)
    }
}

impl ErrorType for TestFlash {
    type Error = NorFlashErrorKind;
}

impl ReadNorFlash for TestFlash {
    const READ_SIZE: usize = 1;

    fn read(&mut self, address: u32, output: &mut [u8]) -> Result<(), Self::Error> {
        output.copy_from_slice(&self.bytes[Self::range(address, output.len())?]);
        Ok(())
    }

    fn capacity(&self) -> usize {
        FLASH_SIZE
    }
}

impl NorFlash for TestFlash {
    const WRITE_SIZE: usize = 1;
    const ERASE_SIZE: usize = CHILD_JOURNAL_SECTOR_SIZE;

    fn write(&mut self, address: u32, input: &[u8]) -> Result<(), Self::Error> {
        if let Some(remaining) = self.writes_before_failure.as_mut() {
            if *remaining == 0 {
                return Err(NorFlashErrorKind::Other);
            }
            *remaining -= 1;
        }
        for (stored, programmed) in self.bytes[Self::range(address, input.len())?]
            .iter_mut()
            .zip(input)
        {
            if (*stored & *programmed) != *programmed {
                return Err(NorFlashErrorKind::Other);
            }
            *stored &= *programmed;
        }
        Ok(())
    }

    fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
        let length = to.checked_sub(from).ok_or(NorFlashErrorKind::OutOfBounds)?;
        let range = Self::range(from, length as usize)?;
        if range.start >= range.end
            || !range.start.is_multiple_of(Self::ERASE_SIZE)
            || !range.end.is_multiple_of(Self::ERASE_SIZE)
        {
            return Err(NorFlashErrorKind::NotAligned);
        }
        self.bytes[range].fill(0xFF);
        Ok(())
    }
}

fn child(timeout: u8) -> PersistentChild {
    PersistentChild {
        ieee_address: [0x22; 8],
        short_address: 0x2345,
        rx_on_when_idle: false,
        security_capable: true,
        is_router: false,
        end_device_timeout: timeout,
        removal_pending: false,
        removal_attempts: 0,
        reassignment_address: None,
        departure_pending: false,
    }
}

fn table(timeout: u8) -> PersistentChildTable {
    let mut table = PersistentChildTable::new(EXTENDED_PAN_ID);
    table.push(child(timeout)).unwrap();
    table
}

fn journal(flash: TestFlash) -> ChildTableJournal<TestFlash> {
    ChildTableJournal::new(flash, 0, CHILD_JOURNAL_SECTOR_SIZE as u32)
}

#[test]
fn power_loss_before_commit_preserves_the_last_durable_child_table() {
    let first = table(8);
    let mut store = journal(TestFlash::erased());
    store.store(&first).unwrap();

    let mut flash = store.into_storage();
    flash.writes_before_failure = Some(1);
    let mut interrupted = journal(flash);
    assert_eq!(
        interrupted.store(&table(14)),
        Err(ChildStoreError::Hardware)
    );

    let mut reopened = journal(interrupted.into_storage());
    assert_eq!(reopened.load(), Ok(Some(first)));
}

#[test]
fn corrupt_newest_record_falls_back_to_the_previous_generation() {
    let first = table(8);
    let second = table(14);
    let mut store = journal(TestFlash::erased());
    store.store(&first).unwrap();
    store.store(&second).unwrap();

    let mut flash = store.into_storage();
    flash.bytes[CHILD_JOURNAL_SLOT_SIZE + 20] ^= 1;

    let mut reopened = journal(flash);
    assert_eq!(reopened.load(), Ok(Some(first)));
}
