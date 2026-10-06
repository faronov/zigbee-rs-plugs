//! Wear-bounded endpoint-1 plug-state persistence.

use zigbee_plug_core::PlugStateRecord;
use zigbee_runtime::nv_storage::{NvError, NvItemId, NvStorage};

const SLOT: NvItemId = NvItemId::AppEndpoint1;

pub const APP_STATE_CHECKPOINT_INTERVAL_MS: u32 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppStateRestore {
    pub relay_on: bool,
    pub energy_uwh: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointOutcome {
    Unchanged,
    Written,
}

/// State needed to bound application-NV writes.
pub struct AppStateCheckpoint {
    sequence: u32,
    last_written_ms: u32,
    last_written_relay: bool,
    has_written: bool,
}

impl AppStateCheckpoint {
    /// Restore the exact endpoint-1 record used by existing plug images.
    ///
    /// Missing, malformed, or checksum-invalid records are the erased/first
    /// boot case. Hardware and capacity errors remain fatal and are returned
    /// to the composition root.
    pub fn restore<N: NvStorage>(store: &mut N) -> Result<(Self, AppStateRestore), NvError> {
        let mut bytes = [0u8; PlugStateRecord::LEN];
        let record = match store.read(SLOT, &mut bytes) {
            Ok(length) if length == PlugStateRecord::LEN => PlugStateRecord::decode(bytes).ok(),
            Ok(_) | Err(NvError::NotFound | NvError::Corrupt) => None,
            Err(error) => return Err(error),
        };

        let (sequence, restored) = match record {
            Some(record) => (
                record.sequence,
                AppStateRestore {
                    relay_on: record.relay_on,
                    energy_uwh: record.energy_uwh,
                },
            ),
            None => (
                0,
                AppStateRestore {
                    relay_on: false,
                    energy_uwh: 0,
                },
            ),
        };

        Ok((
            Self {
                sequence,
                last_written_ms: 0,
                last_written_relay: restored.relay_on,
                has_written: false,
            },
            restored,
        ))
    }

    pub fn maybe_write<N: NvStorage>(
        &mut self,
        store: &mut N,
        now_ms: u32,
        relay_on: bool,
        energy_uwh: u64,
    ) -> Result<CheckpointOutcome, NvError> {
        let relay_changed = relay_on != self.last_written_relay;
        let interval_elapsed = !self.has_written
            || now_ms.wrapping_sub(self.last_written_ms) >= APP_STATE_CHECKPOINT_INTERVAL_MS;
        if !relay_changed && !interval_elapsed {
            return Ok(CheckpointOutcome::Unchanged);
        }

        self.sequence = self.sequence.wrapping_add(1);
        let record = PlugStateRecord::new(self.sequence, relay_on, energy_uwh);
        store.write(SLOT, &record.encode())?;
        self.last_written_ms = now_ms;
        self.last_written_relay = relay_on;
        self.has_written = true;
        Ok(CheckpointOutcome::Written)
    }

    pub fn write_relay_off<N: NvStorage>(
        &mut self,
        store: &mut N,
        now_ms: u32,
        energy_uwh: u64,
    ) -> Result<CheckpointOutcome, NvError> {
        self.maybe_write(store, now_ms, false, energy_uwh)
    }

    pub const fn sequence(&self) -> u32 {
        self.sequence
    }
}
