//! Narrow platform capabilities used by the shared plug application.

use zigbee_plug_controller::NetworkStatus;

use crate::meter::MeterFault;

/// A local relay choice emitted by the Timer1/button service.
///
/// The sequence is acknowledged only after the application has copied the
/// selected state into the ZCL OnOff cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalRelaySelection {
    pub sequence: u32,
    pub relay_on: bool,
}

/// Relay/protection state to apply to fitted outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayCommand {
    pub relay_on: bool,
    /// A latched protection/meter fault; a short press acknowledges it.
    pub safety_tripped: bool,
    /// Prevent physical energization while preserving pending logical state.
    pub relay_inhibited: bool,
}

/// Timer/button/output boundary supplied by a board composition.
pub trait LocalControl {
    fn set_network_status(&mut self, status: NetworkStatus);

    fn take_local_selection(&mut self, last_sequence: &mut u32) -> Option<LocalRelaySelection>;

    fn acknowledge_local_selection(&mut self, sequence: u32);

    fn take_clear_trip_requested(&mut self) -> bool;

    fn take_factory_reset_requested(&mut self) -> bool;

    /// Publish whether the node is currently joined.
    ///
    /// The button service uses it to decide whether a short press toggles
    /// the relay (joined) or requests commissioning (not joined).
    fn set_network_joined(&mut self, joined: bool);

    /// Consume a short press that asked the network frontend to start
    /// Network Steering.
    fn take_commissioning_requested(&mut self) -> bool;

    /// Consume a timeout raised by the interrupt-driven physical watchdog.
    fn take_meter_timeout(&mut self) -> Option<MeterFault>;

    /// Arm the physical relay watchdog for a relative duration in the local
    /// interrupt clock domain.
    fn arm_meter_timeout(&mut self, timeout_ms: u32, fault: MeterFault);

    fn disarm_meter_timeout(&mut self);

    /// Release only the sticky inhibit installed by a consumed meter timeout.
    fn release_meter_timeout_inhibit(&mut self);

    /// Release only the sticky inhibit installed by a consumed long press.
    fn release_factory_reset_inhibit(&mut self);

    fn apply_relay(&mut self, command: RelayCommand);

    /// Physically de-energize the fitted relay before returning.
    fn force_relay_off_sync(&mut self);
}

/// Wrapping millisecond clock used for OnOff and checkpoint scheduling.
pub trait PlugClock {
    fn now_ms(&mut self) -> u32;
}
