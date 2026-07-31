//! Production TLSR8258 smart-plug router loop for the four BL0942-metered
//! products (`tz3000-gjnozsaz-1m`, `tz3000-gjnozsaz-512k`,
//! `tz3000-w0qqde0g`, `tz3000-zloso4jk`).
//!
//! Adapted from `zigbee-rs`'s own `examples/telink-tlsr8258-router`
//! reference loop: persistent `ZigbeeNode`, MainsSinglePhase power source,
//! endpoint 1 MAINS_POWER_OUTLET, start/resume/recommission handling,
//! default reporting, and the radio-first combined ISR (`main.rs`). This
//! module deliberately omits that reference's `JoinMetrics` diagnostics
//! telemetry — it is out of scope for a product firmware image and would
//! only add unused RAM/flash here.
//!
//! EXPERIMENTAL: not run on TLSR8258 hardware. See this crate's `README.md`.

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
use tlsr8258_hal::uart::{Config, Parity, StopBits, Uart};
use tlsr8258_ts011f_bl0942::BoardResources;

use crate::bl0942_task::{self, Bl0942Task};
use crate::persistence::Checkpoint;
use crate::router_support::{self, LoopControl};

#[cfg(feature = "tz3000-gjnozsaz-1m")]
use tz3000_gjnozsaz_1m as product;
#[cfg(feature = "tz3000-gjnozsaz-512k")]
use tz3000_gjnozsaz_512k as product;
#[cfg(feature = "tz3000-w0qqde0g")]
use tz3000_w0qqde0g as product;
#[cfg(feature = "tz3000-zloso4jk")]
use tz3000_zloso4jk as product;

const ENDPOINT: u8 = 1;
const MAX_RX_SLICE_US: u32 = 20_000;
const JOIN_RETRY_MIN_MS: u32 = 5_000;
const JOIN_RETRY_MAX_MS: u32 = 60_000;
const BUTTON_DEBOUNCE_MS: u32 = 30;
const ONE_SECOND_TICKS: u32 = timer::TICKS_PER_MS * 1_000;
const HUNDRED_MS_TICKS: u32 = timer::TICKS_PER_MS * 100;

/// UART system clock: the TLSR8258 always runs its peripheral clock at
/// 24 MHz after `tlsr8258_hal::clocks::init()` (see `timer::TICKS_PER_MS`,
/// which encodes the same 24 MHz figure); there is no separate,
/// independently confirmed constant for it in this HAL, so this is derived
/// from that one instead of duplicating the magic number.
const SYSTEM_CLOCK_HZ: u32 = timer::TICKS_PER_MS * 1_000;

fn uart_config(pins: tlsr8258_ts011f_bl0942::Bl0942Pins) -> Config {
    Config {
        tx: pins.tx,
        rx: pins.rx,
        baud_rate: 4_800,
        system_clock_hz: SYSTEM_CLOCK_HZ,
        parity: Parity::None,
        stop_bits: StopBits::One,
        // No board evidence of RTS/CTS pins wired for the BL0942 UART
        // link on any of these products: leave hardware flow control
        // disabled rather than guessing at pin assignments.
        rts: None,
        cts: None,
    }
}

fn apply_outputs(relay: &Pin, led: &Pin, state: zigbee_plug_controller::RelayLedState) {
    tlsr8258_ts011f_bl0942::set_relay(relay, state.relay_on);
    tlsr8258_ts011f_bl0942::set_led(led, state.led_on);
}

/// Spin forever with the LED forced on, signaling an unrecoverable startup
/// or runtime failure. There is no logging transport on this firmware, so
/// this is the entire diagnostic surface for a fatal condition.
fn fail(relay: &Pin, led: &Pin) -> ! {
    tlsr8258_ts011f_bl0942::set_relay(relay, false);
    tlsr8258_ts011f_bl0942::set_led(led, true);
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

    // One-time full destructure: every field is moved out simultaneously
    // (not partially), so this remains legal even though several fields
    // below (`flash`, `metering`, `uart`) are each consumed exactly once
    // by storage/UART setup afterward. `relay`/`led`/`button` stay as
    // individually owned `Pin`s and are driven through
    // `tlsr8258_ts011f_bl0942`'s free functions for the rest of this loop
    // instead of through `&BoardResources` methods, which could no longer
    // be called once any other field is moved out.
    let BoardResources {
        relay,
        led,
        button,
        metering: metering_pins,
        flash,
        uart: uart_peripheral,
        adc,
        flash_voltage_pin,
    } = resources;

    // See `router_support::mac_for_product`'s doc comment: this uses the
    // geometry-aware factory-identity primitives
    // (`FlashGeometry::from_capacity`, `TelinkMac::new_for_flash_geometry`)
    // and fails closed (spins with the relay off, LED on) if this
    // product's capacity is unsupported or the fitted flash's JEDEC
    // geometry does not match it — never a fabricated or wrong-sector
    // identity. No per-product EUI byte offset is applied.
    let (mac, ieee_address) = match router_support::mac_for_product(product::PRODUCT.flash.capacity)
    {
        Some(pair) => pair,
        None => fail(&relay, &led),
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
        fail(&relay, &led);
    }

    let (mut app_nv, mut security_store) = match product::storage::open_storage(flash) {
        Ok(pair) => pair,
        Err(_) => fail(&relay, &led),
    };
    let (mut checkpoint, restored) = Checkpoint::restore(&mut app_nv);

    let plug = match ZigbeePlug::new(SmartPlugReporting::default()) {
        Ok(plug) => plug,
        Err(_) => fail(&relay, &led),
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
        fail(&relay, &led);
    }

    let mut node = ZigbeeNode::new(device, &mut security_store, &mut profile);

    let mut controller = PlugController::new(
        PlugSettings::default(),
        ProtectionConfig::default(),
        BUTTON_DEBOUNCE_MS,
    );
    let state = controller.apply_startup(node.profile_mut().component_mut(), restored.relay_on);
    apply_outputs(&relay, &led, state);

    let uart = match Uart::new(uart_peripheral, uart_config(metering_pins)) {
        Ok(uart) => uart,
        Err(_) => fail(&relay, &led),
    };
    let mut clock = TickMillis::new(timer::TICKS_PER_MS, timer::now_ticks())
        .expect("Timer0 has a nonzero tick rate");
    let mut metering = Bl0942Task::new(uart, clock.millis());
    metering.restore_energy_uwh(restored.energy_uwh);

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
                Err(_) => fail(&relay, &led),
            }
        }

        if node.configure_default_reporting().is_err() {
            fail(&relay, &led);
        }

        let mut tick_anchor = tlsr8258_hal::timer::now_ticks();
        let mut hundred_ms_anchor = tick_anchor;
        let mut rx_slice_us = MAX_RX_SLICE_US;

        loop {
            let mut event = None;
            match tlsr8258_rt::block_on(node.device_mut().receive_timeout(rx_slice_us)) {
                Ok(indication) => match tlsr8258_rt::block_on(node.process_incoming(&indication)) {
                    Ok(stack_event) => event = stack_event,
                    Err(_) => fail(&relay, &led),
                },
                Err(MacError::NoData) => {}
                Err(_) => fail(&relay, &led),
            }

            if let Some(stack_event) = event {
                match tlsr8258_rt::block_on(router_support::apply_stack_event(
                    &mut node,
                    stack_event,
                )) {
                    LoopControl::Continue => {}
                    LoopControl::Recommission => continue 'commission,
                    LoopControl::Fatal => fail(&relay, &led),
                }
            }

            // Reconcile the physical relay/LED after every processed
            // incoming frame, per this firmware's required behavior.
            let state = controller.reconcile(node.profile_mut().component_mut());
            apply_outputs(&relay, &led, state);

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
                        LoopControl::Fatal => fail(&relay, &led),
                    }
                }
                Err(_) => fail(&relay, &led),
            }

            // Local button: debounce, toggle-or-clear-latch, reconcile.
            let pressed = tlsr8258_ts011f_bl0942::button_pressed(&button);
            let state =
                controller.on_button_sample(node.profile_mut().component_mut(), now_ms, pressed);
            apply_outputs(&relay, &led, state);

            // ZCL On/Off's mandatory ~100 ms timers, decoupled from the
            // BDB `tick()` cadence above.
            if now.wrapping_sub(hundred_ms_anchor) >= HUNDRED_MS_TICKS {
                hundred_ms_anchor = hundred_ms_anchor.wrapping_add(HUNDRED_MS_TICKS);
                let state = controller.tick_100ms(node.profile_mut().component_mut());
                apply_outputs(&relay, &led, state);
            }

            // Bounded metering poll; never blocks.
            if let bl0942_task::Outcome::Sample(sample) = metering.poll(now_ms) {
                let _ = controller.on_sample(node.profile_mut().component_mut(), now_ms, sample);
                let state = controller.reconcile(node.profile_mut().component_mut());
                apply_outputs(&relay, &led, state);
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
                .maybe_write(&mut app_nv, now_ms, relay_on, metering.total_energy_uwh())
                .is_err()
            {
                fail(&relay, &led);
            }
        }
    }
}
