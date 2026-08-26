//! Bounded 1 s-window BL0937 pulse-capture metering task.
//!
//! Owns the CF/CF1 capture channels and the SEL output pin, and drives
//! [`bl0937::Bl0937`] from fixed windows measured against
//! [`tlsr8258_hal::timer::now_ticks`]. Every method is a single bounded
//! poll: event draining is capped at [`MAX_EVENTS_PER_POLL`] per call, and
//! there is no unbounded wait anywhere in this module.
//!
//! # Calibration
//!
//! Calibration and SEL polarity are supplied by the compile-time product
//! selection. The Zbeacon product uses the gain/scaler record recovered from
//! its stock flash; the legacy product retains the repository's reference
//! component calibration. Neither should be treated as production-calibrated
//! until checked against a known load on its physical board.
//!
//! # Fail-safe overflow handling
//!
//! [`tlsr8258_hal::capture::overflow_count`] increasing during a
//! measurement window means the software event queue dropped at least one
//! hardware edge — the pulse counts accumulated for that window are an
//! undercount, not a valid sample. This task has no logging transport to
//! otherwise surface that condition, so it fails safe by discarding the
//! whole window (see [`MeterServiceOutcome::InputDropped`]) rather than feeding a
//! silently-wrong sample to the controller/protection engine.

use plug_router_app::{MeterService, MeterServiceOutcome};
use tlsr8258_hal::capture::{self, CaptureError};
use tlsr8258_hal::gpio::{GpioIrqSource, Pin};
use tlsr8258_hal::timer;
use zigbee_plug_core::ElectricalSample;

use bl0937::{Bl0937, Calibration, MeasurementMode, PulseWindow, SelPolarity};

/// Capture channel indices, matching this task's [`capture::configure_channel`]
/// calls in [`Bl0937Task::new`]. Not `bl0937`/`tlsr8258_hal` constants —
/// this module's own bookkeeping.
const CHANNEL_CF: usize = 0;
const CHANNEL_CF1: usize = 1;

/// Fixed measurement window duration: 1 second, matching this driver
/// family's documented pulse rates (tens of Hz) and this repository's
/// reference component calibration.
const WINDOW_TICKS: u32 = timer::TICKS_PER_MS * 1_000;

/// Capture events drained from the shared queue per [`Bl0937Task::poll`]
/// call. Bounded so one poll can never spend unbounded time; comfortably
/// above the tens-of-Hz pulse rate this driver family targets over one
/// window.
const MAX_EVENTS_PER_POLL: u16 = 256;

/// Switch CF1 between current- and voltage-sensing every two 1 s windows —
/// the same cadence exercised by this repository's `bl0937` driver tests.
const WINDOWS_PER_MODE: u8 = 2;

pub struct Bl0937Task {
    // Kept for the task's lifetime even though `capture::configure_channel`
    // only borrows them at setup: the physical CF/CF1 lines are exclusively
    // owned by this task for as long as it runs, matching this crate's
    // singleton-ownership convention.
    _cf: Pin,
    _cf1: Pin,
    sel: Pin,
    driver: Bl0937,
    window_start_ticks: u32,
    overflow_baseline: u32,
    cf_pulses: u32,
    cf1_pulses: u32,
    /// Last known-valid measurement of each kind. BL0937 alternates CF1
    /// between current and voltage sensing via SEL (see the module docs),
    /// so a completed window only ever refreshes one of these two — the
    /// other is carried forward from its last valid window, accepting the
    /// resulting skew between the two readings as an inherent property of
    /// this metering IC, not a bug in this task.
    last_voltage_mv: u32,
    last_current_ma: u32,
}

impl Bl0937Task {
    /// Arm the CF (Primary)/CF1 (Risc0) capture channels and build the
    /// [`bl0937::Bl0937`] driver. `cf`/`cf1`/`sel` must already be
    /// configured as inputs/output respectively by
    /// `BoardResources::initialize_safe` before this call — this task only
    /// drives `sel`'s level going forward, it does not itself configure
    /// its output-enable/function mux.
    pub fn new(
        cf: Pin,
        cf1: Pin,
        sel: Pin,
        calibration: Calibration,
        sel_polarity: SelPolarity,
    ) -> Result<Self, CaptureError> {
        capture::configure_channel(CHANNEL_CF, &cf, GpioIrqSource::Primary)?;
        capture::configure_channel(CHANNEL_CF1, &cf1, GpioIrqSource::Risc0)?;

        let driver = Bl0937::new(
            calibration,
            MeasurementMode::Current,
            sel_polarity,
            WINDOWS_PER_MODE,
        )
        .expect("WINDOWS_PER_MODE >= 2");

        tlsr8258_hal::gpio::write(&sel, driver.sel_high());

        Ok(Self {
            _cf: cf,
            _cf1: cf1,
            sel,
            driver,
            window_start_ticks: timer::now_ticks(),
            overflow_baseline: capture::overflow_count(),
            cf_pulses: 0,
            cf1_pulses: 0,
            last_voltage_mv: 0,
            last_current_ma: 0,
        })
    }

    /// Restore the durable lifetime energy total read back from NV before
    /// this task's first [`Self::poll`].
    pub fn restore_energy_uwh(&mut self, total_uwh: u64) {
        self.driver.restore_total_energy_uwh(total_uwh);
    }

    /// Current monotonic lifetime energy total, for wear-bounded
    /// persistence checkpoints.
    pub fn total_energy_uwh(&self) -> u64 {
        self.driver.total_energy_uwh()
    }

    /// One bounded poll: drains queued capture events (bounded), and, once
    /// a full 1 s window has elapsed, converts the accumulated pulse
    /// counts into a sample (or discards them).
    pub fn poll(&mut self) -> MeterServiceOutcome {
        self.drain_events();

        let now = timer::now_ticks();
        let elapsed = capture::elapsed_ticks(self.window_start_ticks, now);
        if elapsed < WINDOW_TICKS {
            return MeterServiceOutcome::Idle;
        }
        // Consume the complete aggregate interval and start the next window
        // at the current hardware time. Advancing by only one nominal window
        // would manufacture empty catch-up samples after a delayed poll.
        self.window_start_ticks = now;

        let overflow_now = capture::overflow_count();
        let window_valid = overflow_now == self.overflow_baseline;
        self.overflow_baseline = overflow_now;

        let cf_pulses = self.cf_pulses;
        let cf1_pulses = self.cf1_pulses;
        self.cf_pulses = 0;
        self.cf1_pulses = 0;

        if !window_valid {
            return MeterServiceOutcome::InputDropped;
        }

        // `elapsed` is always `>= WINDOW_TICKS > 0`, so `duration_us` here
        // is always nonzero and `process_window` cannot return
        // `SampleError::ZeroDuration` — but the `Err` arm is still handled
        // explicitly rather than assumed away, per this crate's
        // fail-explicit convention.
        let duration_us = elapsed / timer::TICKS_PER_US;
        match self.driver.process_window(PulseWindow {
            cf_pulses,
            cf1_pulses,
            duration_us,
        }) {
            Ok(result) => {
                if let Some(level) = result.next_sel_high {
                    tlsr8258_hal::gpio::write(&self.sel, level);
                }
                MeterServiceOutcome::Sample(self.sample_from_result(result))
            }
            Err(_) => MeterServiceOutcome::Idle,
        }
    }

    fn sample_from_result(&mut self, result: bl0937::WindowResult) -> ElectricalSample {
        if let Some(current_ma) = result.measurement.current_ma {
            self.last_current_ma = clamp_u32(current_ma);
        }
        if let Some(voltage_mv) = result.measurement.voltage_mv {
            self.last_voltage_mv = clamp_u32(voltage_mv);
        }
        ElectricalSample {
            voltage_mv: self.last_voltage_mv,
            current_ma: self.last_current_ma,
            active_power_mw: clamp_i32(result.measurement.power_mw),
            // BL0937 measures power/current/voltage magnitude from pulse
            // trains; it has no mains-frequency/phase measurement path.
            frequency_millihz: None,
            total_energy_uwh: result.total_energy_uwh,
        }
    }

    fn drain_events(&mut self) {
        for _ in 0..MAX_EVENTS_PER_POLL {
            let Some(event) = capture::take_event() else {
                break;
            };
            match event.channel as usize {
                CHANNEL_CF => self.cf_pulses = self.cf_pulses.saturating_add(1),
                CHANNEL_CF1 => self.cf1_pulses = self.cf1_pulses.saturating_add(1),
                _ => {}
            }
        }
    }
}

impl MeterService for Bl0937Task {
    fn restore_energy_uwh(&mut self, total_uwh: u64) {
        Bl0937Task::restore_energy_uwh(self, total_uwh);
    }

    fn total_energy_uwh(&self) -> u64 {
        Bl0937Task::total_energy_uwh(self)
    }

    fn service(&mut self, _now_ms: u32) -> MeterServiceOutcome {
        self.poll()
    }
}

fn clamp_u32(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

fn clamp_i32(value: u64) -> i32 {
    value.min(u64::from(i32::MAX as u32)) as i32
}
