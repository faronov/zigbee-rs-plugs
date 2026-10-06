//! TLSR8258 adapters for the cross-platform plug application.

use plug_router_app::{LocalControl, LocalRelaySelection, MeterFault, PlugClock, RelayCommand};
use zigbee_plug_controller::NetworkStatus;
use zigbee_plug_core::TickMillis;

use crate::local_control;

pub struct TlsrClock {
    ticks: TickMillis,
}

impl TlsrClock {
    pub fn new() -> Self {
        Self {
            ticks: TickMillis::new(
                tlsr8258_hal::timer::TICKS_PER_MS,
                tlsr8258_hal::timer::now_ticks(),
            )
            .expect("Timer0 has a nonzero tick rate"),
        }
    }

    #[cfg(any(
        feature = "tz3000-gjnozsaz-1m",
        feature = "tz3000-gjnozsaz-512k",
        feature = "tz3000-w0qqde0g",
        feature = "tz3000-zloso4jk",
    ))]
    pub fn millis(&mut self) -> u32 {
        self.now_ms()
    }
}

impl PlugClock for TlsrClock {
    fn now_ms(&mut self) -> u32 {
        self.ticks.update(tlsr8258_hal::timer::now_ticks())
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct TlsrLocalControl;

impl LocalControl for TlsrLocalControl {
    fn set_network_status(&mut self, status: NetworkStatus) {
        local_control::set_network_status(status);
    }

    fn take_local_selection(&mut self, last_sequence: &mut u32) -> Option<LocalRelaySelection> {
        let relay_on = local_control::take_local_relay_change(last_sequence)?;
        Some(LocalRelaySelection {
            sequence: *last_sequence,
            relay_on,
        })
    }

    fn acknowledge_local_selection(&mut self, sequence: u32) {
        local_control::acknowledge_local_relay_change(sequence);
    }

    fn take_clear_trip_requested(&mut self) -> bool {
        local_control::take_clear_trip_requested()
    }

    fn take_factory_reset_requested(&mut self) -> bool {
        local_control::take_factory_reset_requested()
    }

    fn set_network_joined(&mut self, joined: bool) {
        local_control::set_network_joined(joined);
    }

    fn take_commissioning_requested(&mut self) -> bool {
        local_control::take_commissioning_requested()
    }

    fn take_meter_timeout(&mut self) -> Option<MeterFault> {
        local_control::take_meter_timeout()
    }

    fn arm_meter_timeout(&mut self, timeout_ms: u32, fault: MeterFault) {
        local_control::arm_meter_timeout(timeout_ms, fault);
    }

    fn disarm_meter_timeout(&mut self) {
        local_control::disarm_meter_timeout();
    }

    fn release_meter_timeout_inhibit(&mut self) {
        local_control::release_meter_timeout_inhibit();
    }

    fn release_factory_reset_inhibit(&mut self) {
        local_control::release_factory_reset_inhibit();
    }

    fn apply_relay(&mut self, command: RelayCommand) {
        local_control::set_safety_tripped(command.safety_tripped);
        local_control::set_relay_inhibited(command.relay_inhibited);
        local_control::request_relay(command.relay_on);
    }

    fn force_relay_off_sync(&mut self) {
        local_control::force_relay_off_sync();
    }
}
