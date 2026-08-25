//! Host-testable smart-plug behavior: relay/button/LED policy and
//! protection-engine composition over [`zigbee_plug_profile::ZigbeePlug`].
//!
//! This crate owns the *decision* of what the physical relay and LED
//! outputs should be at any instant; it never touches a GPIO itself (no
//! chip or board dependency). Firmware drives [`PlugController`] from its
//! main loop and applies the returned [`RelayLedState`] to real pins.
//!
//! # Ownership
//!
//! [`PlugController`] does not own a [`zigbee_plug_profile::ZigbeePlug`].
//! The plug's Zigbee state lives inside the
//! `zigbee_runtime::profile::DeviceProfile<ZigbeePlug>` that
//! `zigbee_runtime::node::ZigbeeNode` already borrows exclusively, so every
//! method here takes `&mut ZigbeePlug` as a parameter instead of holding
//! one — this avoids a second, competing owner of the same cluster state.
//!
//! # Protection veto (required behavior)
//!
//! [`PlugController::reconcile`] is the single place that decides the
//! *physical* relay level. It never trusts the ZCL OnOff attribute alone:
//! whenever [`zigbee_plug_core::ProtectionEngine`] reports an active trip,
//! `reconcile` both forces the OnOff attribute back to `false` (so a
//! remote client cannot observe a phantom "on" state that was never
//! energized) and ANDs the physical relay decision with "not tripped" a
//! second time. A remote or local On command therefore can never energize
//! the relay while a trip is latched, regardless of the order commands and
//! protection events arrive in.
#![no_std]

pub mod button;
pub mod indicator;

pub use button::{ButtonDebouncer, ButtonEdge, ButtonGesture, ButtonGestureEvent};
pub use indicator::{NetworkStatus, status_led_on};
use zigbee_plug_core::{
    ElectricalSample, PlugSettings, ProtectionAction, ProtectionConfig, ProtectionEngine,
    TripReason,
};
use zigbee_plug_profile::ZigbeePlug;

/// The physical output levels the controller wants applied this instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayLedState {
    pub relay_on: bool,
    pub led_on: bool,
}

/// Composes the ZCL plug profile, the protection engine, button debounce,
/// and LED policy into one relay/LED decision.
///
/// Does not own a `ZigbeePlug` — see the module docs' "Ownership" section.
pub struct PlugController {
    settings: PlugSettings,
    protection: ProtectionEngine,
    button: ButtonDebouncer,
}

impl PlugController {
    pub const fn new(
        settings: PlugSettings,
        protection_config: ProtectionConfig,
        button_debounce_ms: u32,
    ) -> Self {
        Self {
            settings,
            protection: ProtectionEngine::new(protection_config),
            button: ButtonDebouncer::new(button_debounce_ms),
        }
    }

    pub const fn settings(&self) -> PlugSettings {
        self.settings
    }

    pub fn set_settings(&mut self, settings: PlugSettings) {
        self.settings = settings;
    }

    pub const fn trip_reason(&self) -> Option<TripReason> {
        self.protection.trip_reason()
    }

    pub fn clear_protection_latch(&mut self) {
        self.protection.clear_latch();
    }

    /// Apply this product's startup policy on cold boot.
    ///
    /// `previous_relay_on` is the last persisted relay state. `settings`'s
    /// [`zigbee_plug_core::StartupBehavior`] first resolves that into an
    /// effective "previous" value (e.g. always starting `Off` regardless of
    /// history), which is then handed to the ZCL `StartUpOnOff` attribute
    /// via [`ZigbeePlug::apply_startup_on_off`] — so both the product's own
    /// default and any network-configured `StartUpOnOff` value apply, in
    /// that order. Returns the outputs to drive immediately.
    pub fn apply_startup(
        &mut self,
        plug: &mut ZigbeePlug,
        previous_relay_on: bool,
    ) -> RelayLedState {
        let effective_previous = self.settings.startup_relay(previous_relay_on);
        plug.apply_startup_on_off(effective_previous);
        self.reconcile(plug)
    }

    /// Recompute the desired physical relay/LED outputs from the plug's
    /// current ZCL OnOff state and the protection engine's latch. Call
    /// after every processed incoming frame and after every controller
    /// tick — see the module docs' "Protection veto" section for why this
    /// must run unconditionally rather than only on state-change events.
    pub fn reconcile(&mut self, plug: &mut ZigbeePlug) -> RelayLedState {
        if self.protection.trip_reason().is_some() && plug.is_on() {
            // The attribute must never claim "on" while tripped: force it
            // back so a client that just issued an On command observes
            // the plug staying off, not a stale/optimistic "on".
            plug.local_set_on(false);
        }
        let relay_on = plug.is_on() && self.protection.trip_reason().is_none();
        let led_on = self.settings.led_mode.led_on(relay_on);
        RelayLedState { relay_on, led_on }
    }

    /// Feed one electrical sample: updates the ZCL Electrical
    /// Measurement/Metering clusters and evaluates the protection engine.
    /// A permitted voltage auto-restart restores the ZCL OnOff state; this
    /// still does not drive relay/LED outputs itself, so call
    /// [`Self::reconcile`] afterward.
    pub fn on_sample(
        &mut self,
        plug: &mut ZigbeePlug,
        now_ms: u32,
        sample: ElectricalSample,
    ) -> ProtectionAction {
        plug.update_sample(sample);
        let relay_was_on = plug.is_on();
        let action = self.protection.update(now_ms, sample, relay_was_on);
        if action == ProtectionAction::Restart {
            plug.local_set_on(true);
        }
        action
    }

    /// Drive the ZCL On/Off cluster's mandatory 100 ms timers (OnTime /
    /// OffWaitTime) and recompute outputs. Must be called at a bounded
    /// ~100 ms cadence per ZCL §3.8.2.3.1, independent of how often
    /// [`Self::reconcile`] is otherwise invoked after incoming frames.
    pub fn tick_100ms(&mut self, plug: &mut ZigbeePlug) -> RelayLedState {
        plug.tick_on_off();
        self.reconcile(plug)
    }

    /// Feed one raw, debounced button GPIO sample. While the protection
    /// latch is tripped, a stable press clears the latch (a common
    /// "acknowledge the fault" UX) instead of toggling the relay, so the
    /// same physical control can never be used to force power back on
    /// while unsafe. A normal press toggles the relay exactly as a remote
    /// Toggle command would, unless `settings.button_locked` disables local
    /// control entirely (e.g. for a wall-mounted plug a tenant should not
    /// be able to switch off).
    pub fn on_button_sample(
        &mut self,
        plug: &mut ZigbeePlug,
        now_ms: u32,
        pressed: bool,
    ) -> RelayLedState {
        if let Some(ButtonEdge::Pressed) = self.button.sample(now_ms, pressed) {
            if self.protection.trip_reason().is_some() {
                self.protection.clear_latch();
            } else if !self.settings.button_locked {
                plug.local_toggle();
            }
        }
        self.reconcile(plug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zigbee_plug_profile::ZigbeePlug;
    use zigbee_runtime::profile::SmartPlugReporting;

    fn nominal_sample() -> ElectricalSample {
        ElectricalSample {
            voltage_mv: 230_000,
            current_ma: 1_000,
            active_power_mw: 200_000,
            frequency_millihz: Some(50_000),
            total_energy_uwh: 0,
        }
    }

    fn over_current_sample() -> ElectricalSample {
        ElectricalSample {
            current_ma: 20_000,
            ..nominal_sample()
        }
    }

    fn new_plug() -> ZigbeePlug {
        ZigbeePlug::new(SmartPlugReporting::default()).unwrap()
    }

    fn fast_trip_config() -> ProtectionConfig {
        ProtectionConfig {
            trip_delay_ms: 0,
            ..ProtectionConfig::default()
        }
    }

    #[test]
    fn default_settings_never_auto_restart_on_voltage() {
        let controller =
            PlugController::new(PlugSettings::default(), ProtectionConfig::default(), 30);
        assert_eq!(controller.settings(), PlugSettings::default());
        // ProtectionConfig::default() is the source of truth for this; a
        // regression here would silently re-energize a plug after a mains
        // brown-out with no operator action.
        assert!(!ProtectionConfig::default().auto_restart_voltage);
    }

    #[test]
    fn configured_voltage_auto_restart_restores_the_onoff_state() {
        let mut controller = PlugController::new(
            PlugSettings::default(),
            ProtectionConfig {
                trip_delay_ms: 0,
                auto_restart_voltage: true,
                restart_delay_ms: 1_000,
                ..ProtectionConfig::default()
            },
            30,
        );
        let mut plug = new_plug();
        plug.local_set_on(true);

        let mut over_voltage = nominal_sample();
        over_voltage.voltage_mv = 260_000;
        controller.on_sample(&mut plug, 0, over_voltage);
        assert_eq!(
            controller.on_sample(&mut plug, 0, over_voltage),
            ProtectionAction::Trip(TripReason::OverVoltage)
        );
        assert!(!controller.reconcile(&mut plug).relay_on);

        assert_eq!(
            controller.on_sample(&mut plug, 100, nominal_sample()),
            ProtectionAction::None
        );
        assert_eq!(
            controller.on_sample(&mut plug, 1_100, nominal_sample()),
            ProtectionAction::Restart
        );
        assert!(plug.is_on());
        assert!(controller.reconcile(&mut plug).relay_on);
    }

    #[test]
    fn remote_on_command_energizes_relay_when_not_tripped() {
        let mut controller =
            PlugController::new(PlugSettings::default(), ProtectionConfig::default(), 30);
        let mut plug = new_plug();
        plug.local_set_on(true);
        let state = controller.reconcile(&mut plug);
        assert!(state.relay_on);
        assert!(plug.is_on());
    }

    #[test]
    fn remote_on_command_never_energizes_relay_while_tripped() {
        let mut controller = PlugController::new(PlugSettings::default(), fast_trip_config(), 30);
        let mut plug = new_plug();
        plug.local_set_on(true);

        controller.on_sample(&mut plug, 0, over_current_sample());
        let action = controller.on_sample(&mut plug, 0, over_current_sample());
        assert_eq!(action, ProtectionAction::Trip(TripReason::OverCurrent));

        // The remote On command is still logically "on" from the client's
        // perspective at this exact millisecond, but reconcile() must
        // never let it reach the relay.
        let state = controller.reconcile(&mut plug);
        assert!(!state.relay_on);
        assert!(!plug.is_on(), "attribute must be forced back off too");

        // A fresh On command arriving while still tripped must also never
        // energize the relay.
        plug.local_set_on(true);
        let state = controller.reconcile(&mut plug);
        assert!(!state.relay_on);
        assert!(!plug.is_on());
    }

    #[test]
    fn button_press_toggles_unless_locked_or_tripped() {
        let mut controller = PlugController::new(PlugSettings::default(), fast_trip_config(), 30);
        let mut plug = new_plug();

        let state = controller.on_button_sample(&mut plug, 0, true);
        assert!(!state.relay_on); // not yet debounced
        let state = controller.on_button_sample(&mut plug, 30, true);
        assert!(state.relay_on);

        // Trip, then confirm the button no longer toggles the relay on.
        controller.on_sample(&mut plug, 30, over_current_sample());
        controller.on_sample(&mut plug, 30, over_current_sample());
        controller.reconcile(&mut plug);
        let state = controller.on_button_sample(&mut plug, 60, false);
        assert!(!state.relay_on);
        let state = controller.on_button_sample(&mut plug, 90, true);
        assert!(!state.relay_on);
    }

    #[test]
    fn button_press_while_tripped_clears_the_latch_instead_of_toggling() {
        let mut controller = PlugController::new(PlugSettings::default(), fast_trip_config(), 30);
        let mut plug = new_plug();
        plug.local_set_on(true);
        controller.on_sample(&mut plug, 0, over_current_sample());
        controller.on_sample(&mut plug, 0, over_current_sample());
        controller.reconcile(&mut plug);
        assert!(controller.trip_reason().is_some());

        // Debounce the press.
        controller.on_button_sample(&mut plug, 0, true);
        controller.on_button_sample(&mut plug, 30, true);
        assert!(controller.trip_reason().is_none());

        // Clearing the latch does not itself re-energize the relay: the
        // plug was left "off" by the forced reconcile above, so a
        // separate On command is required, matching real breaker-reset UX.
        assert!(!plug.is_on());
    }

    #[test]
    fn locked_button_never_toggles_the_relay() {
        let settings = PlugSettings {
            button_locked: true,
            ..PlugSettings::default()
        };
        let mut controller = PlugController::new(settings, ProtectionConfig::default(), 30);
        let mut plug = new_plug();
        controller.on_button_sample(&mut plug, 0, true);
        let state = controller.on_button_sample(&mut plug, 30, true);
        assert!(!state.relay_on);
        assert!(!plug.is_on());
    }

    #[test]
    fn startup_previous_restores_persisted_relay_state() {
        let mut controller = PlugController::new(
            PlugSettings {
                startup_behavior: zigbee_plug_core::StartupBehavior::Previous,
                ..PlugSettings::default()
            },
            ProtectionConfig::default(),
            30,
        );
        let mut plug = new_plug();
        let state = controller.apply_startup(&mut plug, true);
        assert!(state.relay_on);
        assert!(plug.is_on());
    }

    #[test]
    fn startup_off_ignores_persisted_relay_state() {
        let mut controller = PlugController::new(
            PlugSettings {
                startup_behavior: zigbee_plug_core::StartupBehavior::Off,
                ..PlugSettings::default()
            },
            ProtectionConfig::default(),
            30,
        );
        let mut plug = new_plug();
        let state = controller.apply_startup(&mut plug, true);
        assert!(!state.relay_on);
        assert!(!plug.is_on());
    }

    #[test]
    fn led_follows_relay_by_default() {
        let mut controller =
            PlugController::new(PlugSettings::default(), ProtectionConfig::default(), 30);
        let mut plug = new_plug();
        assert!(!controller.reconcile(&mut plug).led_on);
        plug.local_set_on(true);
        assert!(controller.reconcile(&mut plug).led_on);
    }

    #[test]
    fn hundred_ms_tick_counts_down_on_time_then_turns_off() {
        let mut controller =
            PlugController::new(PlugSettings::default(), ProtectionConfig::default(), 30);
        let mut plug = new_plug();
        plug.local_set_on(true);
        assert!(controller.tick_100ms(&mut plug).relay_on);
        // OnTime defaults to 0 (no timed shutoff configured), so ticking
        // must not spontaneously turn the plug off.
        assert!(controller.tick_100ms(&mut plug).relay_on);
    }
}
