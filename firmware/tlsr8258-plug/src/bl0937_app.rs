//! Production TLSR8258 smart-plug router loop for the legacy BL0937-metered
//! product (`legacy-bl0937-pd6`).
//!
//! Mirrors [`crate::bl0942_app`]'s structure — persistent `ZigbeeNode`,
//! MainsSinglePhase power source, endpoint 1 MAINS_POWER_OUTLET,
//! start/resume/recommission handling, default reporting, radio-first
//! combined ISR (`main.rs`) — but drives three status LEDs instead of one
//! and polls [`crate::bl0937_task::Bl0937Task`]'s fixed 1 s pulse-capture
//! windows instead of a UART stream.
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
use zigbee_zcl::clusters::basic::PowerSource;

use zigbee_plug_controller::PlugController;
use zigbee_plug_core::{PlugSettings, ProtectionConfig, TickMillis};
use zigbee_plug_profile::ZigbeePlug;

use tlsr8258_hal::gpio::Pin;
use tlsr8258_hal::timer;
use tlsr8258_legacy_bl0937::BoardResources;

use crate::bl0937_task::{self, Bl0937Task};
use crate::persistence::Checkpoint;
use crate::router_support::{self, LoopControl};

use legacy_bl0937_pd6 as product;

const ENDPOINT: u8 = 1;
/// Status LED shown solid on fatal failure (see [`fail`]). Index 0 is this
/// board's primary/red status LED (`BoardResources::leds[0]`, wired to
/// PD7 — see `tlsr8258-legacy-bl0937`'s pin map).
const FAIL_LED_INDEX: usize = 0;
const MAX_RX_SLICE_US: u32 = 20_000;
const JOIN_RETRY_MIN_MS: u32 = 5_000;
const JOIN_RETRY_MAX_MS: u32 = 60_000;
const BUTTON_DEBOUNCE_MS: u32 = 30;
const ONE_SECOND_TICKS: u32 = timer::TICKS_PER_MS * 1_000;
const HUNDRED_MS_TICKS: u32 = timer::TICKS_PER_MS * 100;

fn apply_outputs(relay: &Pin, leds: &[Pin; 3], state: zigbee_plug_controller::RelayLedState) {
    tlsr8258_legacy_bl0937::set_relay(relay, state.relay_on);
    tlsr8258_legacy_bl0937::set_led(leds, 0, state.led_on);
}

/// Spin forever with the primary status LED forced on, signaling an
/// unrecoverable startup or runtime failure. There is no logging transport
/// on this firmware, so this is the entire diagnostic surface for a fatal
/// condition.
fn fail(relay: &Pin, leds: &[Pin; 3]) -> ! {
    tlsr8258_legacy_bl0937::set_relay(relay, false);
    tlsr8258_legacy_bl0937::set_led(leds, FAIL_LED_INDEX, true);
    loop {
        core::hint::spin_loop();
    }
}

pub fn run() -> ! {
    type Device = ZigbeeDevice<TelinkMac>;

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
    // partially, to keep `relay`/`leds`/`button` usable via
    // `tlsr8258_legacy_bl0937`'s free functions for the rest of this loop.
    let BoardResources {
        relay,
        leds,
        button,
        metering,
        flash,
        adc,
        flash_voltage_pin,
    } = resources;

    // See `router_support::mac_for_product`'s doc comment: this uses the
    // geometry-aware factory-identity primitives
    // (`FlashGeometry::from_capacity`, `TelinkMac::new_for_flash_geometry`)
    // and fails closed (spins with the relay off, LED on) if this
    // product's capacity is unsupported or the fitted flash's JEDEC
    // geometry does not match it — never a fabricated or wrong-sector
    // identity. This product uses `TLSR8258_1M_LAYOUT`
    // (`FlashGeometry::MiB1`, factory sector `0xFF000`). No per-product
    // EUI byte offset is applied.
    let (mac, ieee_address) = match router_support::mac_for_product(product::PRODUCT.flash.capacity)
    {
        Some(pair) => pair,
        None => fail(&relay, &leds),
    };

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
        fail(&relay, &leds);
    }

    let (mut app_nv, mut security_store) = match product::storage::open_storage(flash) {
        Ok(pair) => pair,
        Err(_) => fail(&relay, &leds),
    };
    let (mut checkpoint, restored) = Checkpoint::restore(&mut app_nv);

    let plug = match ZigbeePlug::new(SmartPlugReporting::default()) {
        Ok(plug) => plug,
        Err(_) => fail(&relay, &leds),
    };
    let mut profile = plug.into_device_profile(ENDPOINT);

    static mut DEVICE_STORAGE: MaybeUninit<ZigbeeDevice<TelinkMac>> = MaybeUninit::uninit();
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
        .build_into(unsafe { &mut *core::ptr::addr_of_mut!(DEVICE_STORAGE) });

    if device
        .reset_security_state_if_identity_changed(&mut security_store, ieee_address)
        .is_err()
    {
        fail(&relay, &leds);
    }

    let mut node = ZigbeeNode::new(device, &mut security_store, &mut profile);

    let mut controller = PlugController::new(
        PlugSettings::default(),
        ProtectionConfig::default(),
        BUTTON_DEBOUNCE_MS,
    );
    let state = controller.apply_startup(node.profile_mut().component_mut(), restored.relay_on);
    apply_outputs(&relay, &leds, state);

    let mut metering_task = match Bl0937Task::new(metering.cf, metering.cf1, metering.sel) {
        Ok(task) => task,
        Err(_) => fail(&relay, &leds),
    };
    metering_task.restore_energy_uwh(restored.energy_uwh);
    let mut clock = TickMillis::new(timer::TICKS_PER_MS, timer::now_ticks())
        .expect("Timer0 has a nonzero tick rate");

    'commission: loop {
        let mut retry_delay_ms = JOIN_RETRY_MIN_MS;
        loop {
            clock.update(timer::now_ticks());
            let start_result = tlsr8258_rt::block_on(node.start_or_resume());
            clock.update(timer::now_ticks());
            match start_result {
                Ok(_) => break,
                Err(StartError::CommissioningFailed(_)) => {
                    tlsr8258_hal::timer::sleep_ticks(tlsr8258_hal::timer::ms(retry_delay_ms));
                    clock.update(timer::now_ticks());
                    retry_delay_ms = retry_delay_ms.saturating_mul(2).min(JOIN_RETRY_MAX_MS);
                }
                Err(_) => fail(&relay, &leds),
            }
        }

        if node.configure_default_reporting().is_err() {
            fail(&relay, &leds);
        }

        let mut tick_anchor = tlsr8258_hal::timer::now_ticks();
        let mut hundred_ms_anchor = tick_anchor;
        let mut rx_slice_us = MAX_RX_SLICE_US;

        loop {
            let mut event = None;
            match tlsr8258_rt::block_on(node.device_mut().receive_timeout(rx_slice_us)) {
                Ok(indication) => match tlsr8258_rt::block_on(node.process_incoming(&indication)) {
                    Ok(stack_event) => event = stack_event,
                    Err(_) => fail(&relay, &leds),
                },
                Err(MacError::NoData) => {}
                Err(_) => fail(&relay, &leds),
            }

            if let Some(stack_event) = event {
                match tlsr8258_rt::block_on(router_support::apply_stack_event(
                    &mut node,
                    stack_event,
                )) {
                    LoopControl::Continue => {}
                    LoopControl::Recommission => continue 'commission,
                    LoopControl::Fatal => fail(&relay, &leds),
                }
            }

            // Reconcile the physical relay/LED after every processed
            // incoming frame, per this firmware's required behavior.
            let state = controller.reconcile(node.profile_mut().component_mut());
            apply_outputs(&relay, &leds, state);

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
                        LoopControl::Recommission => continue 'commission,
                        LoopControl::Fatal => fail(&relay, &leds),
                    }
                }
                Err(_) => fail(&relay, &leds),
            }

            // Local button: debounce, toggle-or-clear-latch, reconcile.
            let pressed = tlsr8258_legacy_bl0937::button_pressed(&button);
            let state =
                controller.on_button_sample(node.profile_mut().component_mut(), now_ms, pressed);
            apply_outputs(&relay, &leds, state);

            // ZCL On/Off's mandatory ~100 ms timers, decoupled from the
            // BDB `tick()` cadence above.
            if now.wrapping_sub(hundred_ms_anchor) >= HUNDRED_MS_TICKS {
                hundred_ms_anchor = hundred_ms_anchor.wrapping_add(HUNDRED_MS_TICKS);
                let state = controller.tick_100ms(node.profile_mut().component_mut());
                apply_outputs(&relay, &leds, state);
            }

            // Bounded metering poll; never blocks. `Bl0937Task::poll` uses
            // its own internal fixed-window clock (see that module's
            // docs), so it takes no `now_ms` argument.
            match metering_task.poll() {
                bl0937_task::Outcome::Sample(sample) => {
                    let _ =
                        controller.on_sample(node.profile_mut().component_mut(), now_ms, sample);
                    let state = controller.reconcile(node.profile_mut().component_mut());
                    apply_outputs(&relay, &leds, state);
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
                fail(&relay, &leds);
            }
        }
    }
}
