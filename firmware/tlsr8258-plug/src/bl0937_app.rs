//! Production TLSR8258 smart-plug router loop for BL0937-metered products.
//!
//! Mirrors [`crate::bl0942_app`]'s structure — persistent `ZigbeeNode`,
//! MainsSinglePhase power source, endpoint 1 MAINS_POWER_OUTLET,
//! start/resume/recommission handling, default reporting, radio-first
//! combined ISR (`main.rs`) — but polls
//! [`crate::bl0937_task::Bl0937Task`]'s fixed 1 s pulse-capture windows
//! instead of a UART stream.
//!
//! EXPERIMENTAL: not run on TLSR8258 hardware. See this crate's `README.md`
//! and `bl0937_task`'s module docs for the uncalibrated/experimental
//! metering caveat.

use core::mem::MaybeUninit;

use zigbee_mac::{MacError, telink::TelinkMac};
use zigbee_nwk::DeviceType;
use zigbee_runtime::ZigbeeDevice;
use zigbee_runtime::event_loop::{StartError, TickResult};
use zigbee_runtime::node::ZigbeeNode;
use zigbee_runtime::power::PowerMode;
use zigbee_runtime::profile::{ApplicationProfile, SmartPlugReporting};
use zigbee_runtime::role::Router;
use zigbee_zcl::clusters::basic::PowerSource;

use zigbee_plug_controller::{NetworkStatus, PlugController, RelayLedState};
use zigbee_plug_core::{PlugSettings, ProtectionConfig, TickMillis};
use zigbee_plug_profile::ZigbeePlug;

use bl0937::{Calibration, SelPolarity};
use tlsr8258_hal::gpio::Pin;
use tlsr8258_hal::timer;

use board::BoardResources;
#[cfg(feature = "legacy-bl0937-pd6")]
use tlsr8258_legacy_bl0937 as board;
#[cfg(feature = "zbeacon-ts011f-512k")]
use tlsr8258_zbeacon_ts011f_bl0937 as board;

use crate::bl0937_task::{self, Bl0937Task};
use crate::local_control;
use crate::persistence::Checkpoint;
use crate::router_support::{self, LoopControl};

#[cfg(feature = "legacy-bl0937-pd6")]
use legacy_bl0937_pd6 as product;
#[cfg(feature = "zbeacon-ts011f-512k")]
use zbeacon_ts011f_512k as product;

const ENDPOINT: u8 = 1;
const MAX_RX_SLICE_US: u32 = 20_000;
const JOIN_RETRY_MIN_MS: u32 = 5_000;
const JOIN_RETRY_MAX_MS: u32 = 60_000;
const BUTTON_DEBOUNCE_MS: u32 = 30;
const ONE_SECOND_TICKS: u32 = timer::TICKS_PER_MS * 1_000;
const HUNDRED_MS_TICKS: u32 = timer::TICKS_PER_MS * 100;

#[cfg(feature = "legacy-bl0937-pd6")]
fn product_metering_config() -> (Calibration, SelPolarity) {
    let calibration = Calibration::from_components(1_000, 2_351_000, 3_200)
        .expect("legacy reference component constants are valid");
    (calibration, SelPolarity::HighIsCurrent)
}

#[cfg(feature = "zbeacon-ts011f-512k")]
const fn product_metering_config() -> (Calibration, SelPolarity) {
    (product::BL0937_CALIBRATION, product::BL0937_SEL_POLARITY)
}

#[cfg(feature = "legacy-bl0937-pd6")]
fn product_protection_config() -> ProtectionConfig {
    ProtectionConfig::default()
}

#[cfg(feature = "zbeacon-ts011f-512k")]
const fn product_protection_config() -> ProtectionConfig {
    product::PROTECTION_CONFIG
}

fn apply_outputs(controller: &PlugController, state: RelayLedState) {
    local_control::set_safety_tripped(controller.trip_reason().is_some());
    local_control::request_relay(state.relay_on);
}

fn sync_local_control(
    controller: &mut PlugController,
    plug: &mut ZigbeePlug,
    local_relay_sequence: &mut u32,
) {
    if let Some(relay_on) = local_control::take_local_relay_change(local_relay_sequence) {
        plug.local_set_on(relay_on);
        local_control::acknowledge_local_relay_change(*local_relay_sequence);
    }
    if local_control::take_clear_trip_requested() {
        controller.clear_protection_latch();
    }
    let state = controller.reconcile(plug);
    apply_outputs(controller, state);
}

/// Spin forever with the primary status LED forced on, signaling an
/// unrecoverable startup or runtime failure. There is no logging transport
/// on this firmware, so this is the entire diagnostic surface for a fatal
/// condition.
fn fail_before_local_control(relay: &Pin, led: &Pin) -> ! {
    board::set_relay(relay, false);
    board::set_led(led, true);
    loop {
        core::hint::spin_loop();
    }
}

fn fail() -> ! {
    local_control::enter_fault();
    loop {
        core::hint::spin_loop();
    }
}

pub fn run() -> ! {
    // See `bl0942_app::run`: a mains-powered plug is a genuine parent
    // `Router` (`zigbee_runtime::role::Router`), constructed through
    // `build_router_into` on a `ParentMacDriver` backend — not the default
    // `EndDevice` or a forwarding-only `RelayRouter`.
    type Device = ZigbeeDevice<TelinkMac, Router>;

    let resources = match BoardResources::take() {
        Some(resources) => resources,
        None => loop {
            core::hint::spin_loop();
        },
    };
    if resources.initialize_safe().is_err() {
        loop {
            core::hint::spin_loop();
        }
    }

    // One-time full destructure — see `bl0942_app::run`'s equivalent
    // comment for why this must move every field out at once rather than
    // partially, to keep `relay`/`led`/`button` usable via the selected
    // board crate's free functions for the rest of this loop.
    let BoardResources {
        relay,
        led,
        button,
        metering,
        flash,
        aes,
        adc,
        flash_voltage_pin,
        ..
    } = resources;

    // See `router_support::mac_for_product`'s doc comment: this uses the
    // geometry-aware factory-identity primitives
    // (`FlashGeometry::from_capacity`, `TelinkMac::new_for_flash_geometry`)
    // and fails closed (spins with the relay off, LED on) if this
    // product's capacity is unsupported or the fitted flash's JEDEC
    // geometry does not match it — never a fabricated or wrong-sector
    // identity. The selected product supplies either the 512 KiB or 1 MiB
    // geometry and its matching factory sector. No per-product EUI byte
    // offset is applied.
    let (mut mac, ieee_address) =
        match router_support::mac_for_product(product::PRODUCT.flash.capacity) {
            Some(pair) => pair,
            None => fail_before_local_control(&relay, &led),
        };
    if mac.install_aes_engine(aes).is_err() {
        fail_before_local_control(&relay, &led);
    }

    // Install the real Zbit flash-voltage guard before opening/writing any
    // persistent storage. See `router_support::install_flash_voltage_guard`'s
    // doc comment: this replaces `tlsr8258_hal::flash::ensure_safe_flash`'s
    // fail-closed default (which never fabricates a fixed reading) with an
    // actual ADC-sampled PC5 measurement, and fails this boot closed if
    // that cannot be established.
    if !router_support::install_flash_voltage_guard(
        adc,
        flash_voltage_pin,
        product::PRODUCT.flash.capacity,
    ) {
        fail_before_local_control(&relay, &led);
    }

    let (mut app_nv, mut security_store) = match product::storage::open_storage(flash) {
        Ok(pair) => pair,
        Err(_) => fail_before_local_control(&relay, &led),
    };
    let (mut checkpoint, restored) = Checkpoint::restore(&mut app_nv);

    let plug = match ZigbeePlug::new(SmartPlugReporting::default()) {
        Ok(plug) => plug,
        Err(_) => fail_before_local_control(&relay, &led),
    };
    let mut profile = plug.into_device_profile(ENDPOINT);

    static mut DEVICE_STORAGE: MaybeUninit<Device> = MaybeUninit::uninit();
    let device: &mut Device = ZigbeeDevice::builder(mac)
        .device_type(DeviceType::Router)
        .power_mode(PowerMode::AlwaysOn)
        .manufacturer(product::PRODUCT.stock_manufacturer.unwrap_or("zigbee-rs"))
        .model(product::PRODUCT.stock_model)
        .power_source(PowerSource::MainsSinglePhase)
        .endpoint(
            profile.endpoint(),
            profile.profile_id(),
            profile.device_id(),
            |endpoint| profile.configure_endpoint(endpoint),
        )
        .build_router_into(unsafe { &mut *core::ptr::addr_of_mut!(DEVICE_STORAGE) });

    if device
        .reset_security_state_if_identity_changed(&mut security_store, ieee_address)
        .is_err()
    {
        fail_before_local_control(&relay, &led);
    }

    let mut node = ZigbeeNode::new(device, &mut security_store, &mut profile);

    let mut controller = PlugController::new(
        PlugSettings::default(),
        product_protection_config(),
        BUTTON_DEBOUNCE_MS,
    );
    let state = controller.apply_startup(node.profile_mut().component_mut(), restored.relay_on);
    local_control::init(
        relay,
        led,
        button,
        board::set_relay,
        board::set_led,
        board::button_pressed,
        state.relay_on,
    );
    apply_outputs(&controller, state);

    let (metering_calibration, sel_polarity) = product_metering_config();
    let mut metering_task = match Bl0937Task::new(
        metering.cf,
        metering.cf1,
        metering.sel,
        metering_calibration,
        sel_polarity,
    ) {
        Ok(task) => task,
        Err(_) => fail(),
    };
    metering_task.restore_energy_uwh(restored.energy_uwh);
    let mut clock = TickMillis::new(timer::TICKS_PER_MS, timer::now_ticks())
        .expect("Timer0 has a nonzero tick rate");
    let mut local_relay_sequence = 0;

    'commission: loop {
        local_control::set_network_status(NetworkStatus::Searching);
        let mut retry_delay_ms = JOIN_RETRY_MIN_MS;
        loop {
            sync_local_control(
                &mut controller,
                node.profile_mut().component_mut(),
                &mut local_relay_sequence,
            );
            local_control::set_network_status(NetworkStatus::Searching);
            clock.update(timer::now_ticks());
            let start_result = tlsr8258_rt::block_on(node.start_or_resume());
            clock.update(timer::now_ticks());
            sync_local_control(
                &mut controller,
                node.profile_mut().component_mut(),
                &mut local_relay_sequence,
            );
            if local_control::take_factory_reset_requested() {
                node.profile_mut().component_mut().local_set_on(false);
                let state = controller.reconcile(node.profile_mut().component_mut());
                apply_outputs(&controller, state);
                local_control::set_network_status(NetworkStatus::Searching);
                let now_ms = clock.update(timer::now_ticks());
                if checkpoint
                    .write_relay_off(&mut app_nv, now_ms, metering_task.total_energy_uwh())
                    .is_err()
                {
                    fail();
                }
                if tlsr8258_rt::block_on(node.factory_reset()).is_err() {
                    fail();
                }
                continue 'commission;
            }
            match start_result {
                Ok(_) => break,
                Err(StartError::CommissioningFailed(_)) => {
                    local_control::set_network_status(NetworkStatus::Searching);
                    let announce_exhausted = node
                        .device()
                        .steering_diagnostics()
                        .device_annce_exhausted();
                    if announce_exhausted && tlsr8258_rt::block_on(node.factory_reset()).is_err() {
                        fail();
                    }
                    let wait_started = timer::now_ticks();
                    let wait_ticks = timer::ms(retry_delay_ms);
                    while timer::now_ticks().wrapping_sub(wait_started) < wait_ticks {
                        sync_local_control(
                            &mut controller,
                            node.profile_mut().component_mut(),
                            &mut local_relay_sequence,
                        );
                        if local_control::take_factory_reset_requested() {
                            node.profile_mut().component_mut().local_set_on(false);
                            let state =
                                controller.reconcile(node.profile_mut().component_mut());
                            apply_outputs(&controller, state);
                            let now_ms = clock.update(timer::now_ticks());
                            if checkpoint
                                .write_relay_off(
                                    &mut app_nv,
                                    now_ms,
                                    metering_task.total_energy_uwh(),
                                )
                                .is_err()
                            {
                                fail();
                            }
                            if tlsr8258_rt::block_on(node.factory_reset()).is_err() {
                                fail();
                            }
                            continue 'commission;
                        }
                        timer::sleep_ticks(timer::ms(20));
                        clock.update(timer::now_ticks());
                    }
                    retry_delay_ms = retry_delay_ms.saturating_mul(2).min(JOIN_RETRY_MAX_MS);
                }
                Err(_) => fail(),
            }
        }
        local_control::set_network_status(NetworkStatus::Joined);

        if node.configure_default_reporting().is_err() {
            fail();
        }

        let mut tick_anchor = tlsr8258_hal::timer::now_ticks();
        let mut hundred_ms_anchor = tick_anchor;
        let mut rx_slice_us = MAX_RX_SLICE_US;

        loop {
            sync_local_control(
                &mut controller,
                node.profile_mut().component_mut(),
                &mut local_relay_sequence,
            );
            if local_control::take_factory_reset_requested() {
                node.profile_mut().component_mut().local_set_on(false);
                let state = controller.reconcile(node.profile_mut().component_mut());
                apply_outputs(&controller, state);
                local_control::set_network_status(NetworkStatus::Searching);
                let now_ms = clock.update(timer::now_ticks());
                if checkpoint
                    .write_relay_off(&mut app_nv, now_ms, metering_task.total_energy_uwh())
                    .is_err()
                {
                    fail();
                }
                if tlsr8258_rt::block_on(node.factory_reset()).is_err() {
                    fail();
                }
                continue 'commission;
            }

            let mut event = None;
            match tlsr8258_rt::block_on(node.device_mut().receive_timeout(rx_slice_us)) {
                Ok(indication) => match tlsr8258_rt::block_on(node.process_incoming(&indication)) {
                    Ok(stack_event) => event = stack_event,
                    Err(_) => fail(),
                },
                Err(MacError::NoData) => {}
                Err(_) => fail(),
            }

            if let Some(stack_event) = event {
                match tlsr8258_rt::block_on(router_support::apply_stack_event(
                    &mut node,
                    stack_event,
                )) {
                    LoopControl::Continue => {}
                    LoopControl::Recommission => {
                        local_control::set_network_status(NetworkStatus::Searching);
                        continue 'commission;
                    }
                    LoopControl::Fatal => fail(),
                }
            }

            // Reconcile the physical relay/LED after every processed
            // incoming frame, per this firmware's required behavior.
            let state = controller.reconcile(node.profile_mut().component_mut());
            apply_outputs(&controller, state);

            let now = tlsr8258_hal::timer::now_ticks();
            let now_ms = clock.update(now);
            let elapsed = now.wrapping_sub(tick_anchor);
            let elapsed_secs = if elapsed >= ONE_SECOND_TICKS {
                let secs = (elapsed / ONE_SECOND_TICKS).min(u16::MAX as u32) as u16;
                tick_anchor = tick_anchor.wrapping_add(u32::from(secs) * ONE_SECOND_TICKS);
                secs
            } else {
                0
            };

            match tlsr8258_rt::block_on(node.tick(elapsed_secs)) {
                Ok(TickResult::Idle) => rx_slice_us = MAX_RX_SLICE_US,
                Ok(TickResult::RunAgain(delay_ms)) => {
                    rx_slice_us = delay_ms.max(1).saturating_mul(1_000).min(MAX_RX_SLICE_US);
                }
                Ok(TickResult::Event(stack_event)) => {
                    rx_slice_us = MAX_RX_SLICE_US;
                    match tlsr8258_rt::block_on(router_support::apply_stack_event(
                        &mut node,
                        stack_event,
                    )) {
                        LoopControl::Continue => {}
                        LoopControl::Recommission => {
                            local_control::set_network_status(NetworkStatus::Searching);
                            continue 'commission;
                        }
                        LoopControl::Fatal => fail(),
                    }
                }
                Err(_) => fail(),
            }

            // ZCL On/Off's mandatory ~100 ms timers, decoupled from the
            // BDB `tick()` cadence above.
            if now.wrapping_sub(hundred_ms_anchor) >= HUNDRED_MS_TICKS {
                hundred_ms_anchor = hundred_ms_anchor.wrapping_add(HUNDRED_MS_TICKS);
                let state = controller.tick_100ms(node.profile_mut().component_mut());
                apply_outputs(&controller, state);
            }

            // Bounded metering poll; never blocks. `Bl0937Task::poll` uses
            // its own internal fixed-window clock (see that module's
            // docs), so it takes no `now_ms` argument.
            match metering_task.poll() {
                bl0937_task::Outcome::Sample(sample) => {
                    let _ =
                        controller.on_sample(node.profile_mut().component_mut(), now_ms, sample);
                    let state = controller.reconcile(node.profile_mut().component_mut());
                    apply_outputs(&controller, state);
                }
                // Fail safe: an overflow-dropped window contributed no
                // sample. There is no logging transport to otherwise
                // surface this (see `bl0937_task`'s module docs); the next
                // window starts fresh.
                bl0937_task::Outcome::OverflowDropped | bl0937_task::Outcome::Idle => {}
            }

            // Wear-bounded persistence checkpoint (not every loop — see
            // `persistence.rs`). A write error (e.g. `FlashError::
            // VoltageGuardUnavailable` surfacing through `NvError` on a
            // Zbit part with no voltage guard installed) must never be
            // silently discarded — this firmware never fabricates a
            // successful checkpoint, so it fails closed the same way every
            // other storage error in this loop does.
            let relay_on = node.profile_mut().component_mut().is_on();
            if checkpoint
                .maybe_write(
                    &mut app_nv,
                    now_ms,
                    relay_on,
                    metering_task.total_energy_uwh(),
                )
                .is_err()
            {
                fail();
            }
        }
    }
}
