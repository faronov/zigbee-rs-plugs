//! Bounded, nonblocking BL0942 UART metering task.
//!
//! Owns the PB1/PB7 [`tlsr8258_hal::uart::Uart`] plus [`bl0942::StreamParser`]
//! and [`bl0942::EnergyTracker`]. Every method here is a single bounded poll
//! — there are no unbounded waits anywhere in this module, matching this
//! firmware's overall no-blocking-loop convention.
//!
//! # Calibration
//!
//! Uses [`bl0942::Calibration::common_reference_board`] — the repository's
//! own documented reference calibration for a common 1 mOhm shunt/divider
//! board, **not** an individually verified per-unit calibration. Voltage,
//! current, and power readings from this task are only as accurate as that
//! reference component set; treat them as indicative, not calibrated
//! instrument-grade values, until a specific product is measured against a
//! known load on real hardware.

use bl0942::{Calibration, EnergyTracker, FeedResult, RawFrame, StreamParser};
use tlsr8258_hal::uart::Uart;
use zigbee_plug_core::ElectricalSample;

/// BL0942 protocol device address. This firmware only ever drives one
/// metering IC per board, so the fixed single-device address (0) is used;
/// [`bl0942::ConfigError::InvalidAddress`] would only trigger for values
/// above 3, which never occurs here.
const ADDRESS: u8 = 0;

/// How often to request a fresh full packet from the meter. The BL0942
/// free-runs and streams unsolicited full packets too, but explicitly
/// polling keeps the request cadence bounded and independent of the chip's
/// own internal report interval.
const REQUEST_INTERVAL_MS: u32 = 1_000;

/// Bytes drained from the RX FIFO per [`Bl0942Task::poll`] call. Bounded so
/// one poll can never spend unbounded time even under a pathological byte
/// flood; one BL0942 frame is [`bl0942::FRAME_LEN`] (23) bytes, so this
/// comfortably drains more than one frame per poll under normal load.
const MAX_RX_BYTES_PER_POLL: u8 = 64;

/// Consecutive `UartError` results from `try_read`/`try_write` before this
/// task gives up on the current peripheral state and reinitializes it via
/// [`Uart::reset`]. Bounded so a persistently wedged UART cannot spin this
/// task forever without ever recovering.
const MAX_ERROR_STREAK: u8 = 8;

/// This poll's result. Never blocks; the caller decides what to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing new this poll.
    Idle,
    /// A full packet was parsed and converted.
    Sample(ElectricalSample),
    /// The UART was reinitialized after too many consecutive errors. The
    /// caller should treat this as "no sample this poll", not a fatal
    /// condition — the task keeps running afterward.
    ResetRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TxState {
    Idle,
    Sending { bytes: [u8; 2], index: u8 },
}

pub struct Bl0942Task {
    uart: Uart,
    parser: StreamParser,
    calibration: Calibration,
    energy: EnergyTracker,
    tx: TxState,
    next_request_at_ms: u32,
    error_streak: u8,
}

impl Bl0942Task {
    /// `now_ms` is the current bounded millisecond clock; the first request
    /// is scheduled immediately (`next_request_at_ms = now_ms`).
    pub fn new(uart: Uart, now_ms: u32) -> Self {
        let calibration = Calibration::common_reference_board();
        Self {
            uart,
            // `ADDRESS` (0) and `calibration.energy_counts_per_kwh` (a
            // documented nonzero reference constant) can never fail these
            // constructors; both are `unwrap`-safe by construction.
            parser: StreamParser::new(ADDRESS).expect("ADDRESS is a valid BL0942 address"),
            calibration,
            energy: EnergyTracker::new(calibration.energy_counts_per_kwh)
                .expect("reference calibration's energy_counts_per_kwh is nonzero"),
            tx: TxState::Idle,
            next_request_at_ms: now_ms,
            error_streak: 0,
        }
    }

    /// Restore the durable lifetime energy total read back from NV before
    /// this task's first [`Self::poll`] — see `persistence.rs`. Re-baselines
    /// the hardware counter tracking on the next observed frame, exactly as
    /// [`EnergyTracker::restore_total_uwh`] documents.
    pub fn restore_energy_uwh(&mut self, total_uwh: u64) {
        self.energy.restore_total_uwh(total_uwh);
    }

    /// Current monotonic lifetime energy total, for wear-bounded
    /// persistence checkpoints.
    pub fn total_energy_uwh(&self) -> u64 {
        self.energy.total_uwh()
    }

    /// One bounded poll: drives the TX request state machine, then drains
    /// up to [`MAX_RX_BYTES_PER_POLL`] bytes into the stream parser. Call
    /// at a steady cadence from the main loop (see `bl0942_app.rs`).
    pub fn poll(&mut self, now_ms: u32) -> Outcome {
        if let Some(outcome) = self.drive_tx(now_ms) {
            return outcome;
        }

        for _ in 0..MAX_RX_BYTES_PER_POLL {
            match self.uart.try_read() {
                Ok(Some(byte)) => {
                    self.error_streak = 0;
                    if let Some(outcome) = self.feed(byte) {
                        return outcome;
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    if self.register_error() {
                        return Outcome::ResetRequested;
                    }
                    break;
                }
            }
        }
        Outcome::Idle
    }

    fn feed(&mut self, byte: u8) -> Option<Outcome> {
        match self.parser.push(byte) {
            FeedResult::Pending => None,
            // A checksum/header parse failure is a protocol-framing event,
            // not a UART hardware error: `StreamParser` has already
            // resynchronized on the next header byte internally, so this
            // does not count toward `error_streak`/`Uart::reset`.
            FeedResult::Error(_) => None,
            FeedResult::Frame(frame) => Some(Outcome::Sample(self.sample_from_frame(frame))),
        }
    }

    fn sample_from_frame(&mut self, frame: RawFrame) -> ElectricalSample {
        let measurement = bl0942::Measurement::from_raw(frame, self.calibration);
        // `counter_energy_uwh` wraps with the BL0942's 24-bit hardware
        // counter and is not a lifetime total (see `Measurement`'s own
        // docs) — only `EnergyTracker::observe`'s monotonic accumulation is
        // used for `ElectricalSample::total_energy_uwh`.
        self.energy.observe(frame.energy_counter);
        ElectricalSample {
            voltage_mv: measurement.voltage_mv,
            current_ma: measurement.current_ma,
            active_power_mw: measurement.active_power_mw,
            frequency_millihz: measurement.frequency_millihz,
            total_energy_uwh: self.energy.total_uwh(),
        }
    }

    /// Advance the TX request state machine. Returns `Some(outcome)` only
    /// if a write error forced an immediate reset (this never happens with
    /// the current `Uart::try_write`, which cannot return `Err`, but is
    /// handled explicitly rather than assumed away).
    fn drive_tx(&mut self, now_ms: u32) -> Option<Outcome> {
        if let TxState::Idle = self.tx {
            // Wrapping-safe "has `now_ms` reached `next_request_at_ms` yet"
            // check: the signed interpretation of the wrapped difference is
            // negative exactly while `next_request_at_ms` is still in the
            // future, and this remains correct across a `u32` millisecond
            // wrap as long as the gap stays well under `i32::MAX`, which a
            // ~1 s request interval always does.
            let due = (now_ms.wrapping_sub(self.next_request_at_ms) as i32) >= 0;
            if !due {
                return None;
            }
            let bytes =
                bl0942::full_packet_request(ADDRESS).expect("ADDRESS is a valid BL0942 address");
            self.tx = TxState::Sending { bytes, index: 0 };
        }

        if let TxState::Sending { bytes, index } = self.tx {
            match self.uart.try_write(bytes[index as usize]) {
                Ok(true) => {
                    self.error_streak = 0;
                    let next_index = index + 1;
                    if next_index as usize == bytes.len() {
                        self.tx = TxState::Idle;
                        self.next_request_at_ms = now_ms.wrapping_add(REQUEST_INTERVAL_MS);
                    } else {
                        self.tx = TxState::Sending {
                            bytes,
                            index: next_index,
                        };
                    }
                }
                Ok(false) => {
                    // Backpressure: TX FIFO is full. Retry next poll —
                    // never block waiting for room.
                }
                Err(_) => {
                    if self.register_error() {
                        return Some(Outcome::ResetRequested);
                    }
                }
            }
        }
        None
    }

    /// Record one UART error. Returns `true` (and resets the peripheral)
    /// once [`MAX_ERROR_STREAK`] consecutive errors have accumulated.
    fn register_error(&mut self) -> bool {
        self.error_streak = self.error_streak.saturating_add(1);
        if self.error_streak >= MAX_ERROR_STREAK {
            // Best-effort: `Uart::reset` now reports failures explicitly
            // (upstream HAL churn), but this call site already has a
            // bounded-retry design for a persistently unhealthy
            // peripheral — falling through still clears the streak and
            // reports `ResetRequested`, so the next `MAX_ERROR_STREAK`
            // consecutive UART errors retries the reset again rather than
            // silently giving up forever on one failed attempt.
            let _ = self.uart.reset();
            self.error_streak = 0;
            self.tx = TxState::Idle;
            true
        } else {
            false
        }
    }
}
