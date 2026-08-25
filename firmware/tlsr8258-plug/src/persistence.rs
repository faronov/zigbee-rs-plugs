//! Wear-bounded restore/checkpoint of [`zigbee_plug_core::PlugStateRecord`]
//! over a product's [`zigbee_plug_storage::flash::ApplicationNv`] log.
//!
//! Shared by both the BL0942 and BL0937 router loops: every product's
//! `open_storage()` returns the same concrete `ApplicationNv`/`SecurityStore`
//! types (see `zigbee-plug-storage`'s `flash.rs`), so this module has no
//! per-product or per-board code at all.

use zigbee_plug_core::PlugStateRecord;
use zigbee_plug_storage::flash::ApplicationNv;
use zigbee_runtime::nv_storage::{NvError, NvItemId, NvStorage};

/// Endpoint-1 plug state lives in the first application-owned NV slot.
/// `zigbee-runtime`'s own `NvItemId` enum already reserves this slot for
/// exactly this purpose; adding a new variant there was unnecessary.
const SLOT: NvItemId = NvItemId::AppEndpoint1;

/// Minimum time between wear-bounded writes: keeps a plug that ticks every
/// 100 ms from writing every tick (the flash's rated endurance is the
/// concern here, not correctness — a crash between checkpoints only loses
/// up to this much energy-accounting precision and, if the relay changed in
/// that window, one restart's worth of startup-policy history).
const MIN_CHECKPOINT_INTERVAL_MS: u32 = 60_000;
/// Also checkpoint promptly on a relay state change, so a crash right after
/// a command does not restart into the wrong relay state — this is the one
/// trigger that bypasses [`MIN_CHECKPOINT_INTERVAL_MS`]. Energy is only
/// ever checkpointed at that bounded interval: a crash between checkpoints
/// only loses up to one interval's worth of energy-accounting precision,
/// which is an acceptable trade against flash wear.

/// Tracks when this device last wrote a checkpoint so [`Checkpoint::maybe_write`]
/// can bound how often it touches flash.
pub struct Checkpoint {
    sequence: u32,
    last_written_ms: u32,
    last_written_relay: bool,
    last_written_energy_uwh: u64,
    has_written: bool,
}

/// The state restored from NV at startup, or product defaults if none was
/// ever persisted (first boot, or a factory-erased part).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestoredState {
    pub relay_on: bool,
    pub energy_uwh: u64,
}

impl Checkpoint {
    /// Read the persisted record before profile startup (requirement:
    /// "Restore before profile startup"). A missing or corrupt record is
    /// not an error the caller must handle specially — it is exactly the
    /// expected first-boot condition, so this returns product defaults
    /// (relay off, zero energy) instead of failing the boot.
    pub fn restore(app_nv: &mut ApplicationNv) -> (Self, RestoredState) {
        let mut buf = [0u8; PlugStateRecord::LEN];
        let restored = match app_nv.read(SLOT, &mut buf) {
            Ok(len) if len == PlugStateRecord::LEN => PlugStateRecord::decode(buf).ok(),
            _ => None,
        };

        let (sequence, state) = match restored {
            Some(record) => (
                record.sequence,
                RestoredState {
                    relay_on: record.relay_on,
                    energy_uwh: record.energy_uwh,
                },
            ),
            None => (
                0,
                RestoredState {
                    relay_on: false,
                    energy_uwh: 0,
                },
            ),
        };

        (
            Self {
                sequence,
                last_written_ms: 0,
                last_written_relay: state.relay_on,
                last_written_energy_uwh: state.energy_uwh,
                has_written: false,
            },
            state,
        )
    }

    /// Write a checkpoint only if wear-bounded conditions are met: this is
    /// not called from every 100 ms tick unconditionally (see the module
    /// docs). `now_ms` must be the same wrapping millisecond clock used
    /// elsewhere in the firmware.
    pub fn maybe_write(
        &mut self,
        app_nv: &mut ApplicationNv,
        now_ms: u32,
        relay_on: bool,
        energy_uwh: u64,
    ) -> Result<bool, NvError> {
        let relay_changed = relay_on != self.last_written_relay;
        let interval_elapsed = !self.has_written
            || now_ms.wrapping_sub(self.last_written_ms) >= MIN_CHECKPOINT_INTERVAL_MS;

        if !relay_changed && !interval_elapsed {
            return Ok(false);
        }

        self.sequence = self.sequence.wrapping_add(1);
        let record = PlugStateRecord::new(self.sequence, relay_on, energy_uwh);
        app_nv.write(SLOT, &record.encode())?;

        self.last_written_ms = now_ms;
        self.last_written_relay = relay_on;
        self.last_written_energy_uwh = energy_uwh;
        self.has_written = true;
        Ok(true)
    }

    /// Persist the relay-off state before a local network factory reset.
    pub fn write_relay_off(
        &mut self,
        app_nv: &mut ApplicationNv,
        now_ms: u32,
        energy_uwh: u64,
    ) -> Result<(), NvError> {
        self.maybe_write(app_nv, now_ms, false, energy_uwh)
            .map(|_| ())
    }
}
