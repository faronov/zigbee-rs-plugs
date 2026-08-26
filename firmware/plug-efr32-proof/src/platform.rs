//! Fixed BRD4181A GPIO, local-control, clock, and reset adapters.
//!
//! The 10 ms button/output service runs from SysTick so local control remains
//! independent of bounded Zigbee operations. It deliberately mirrors the
//! stock plug's debounced short-press/long-press and LED policy.

use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering},
};

use efr32mg21_brd4181a_plug::{RelayOutput, StatusLed, UserButton};
use plug_router_app::{LocalControl, LocalRelaySelection, MeterFault, PlugClock, RelayCommand};
use router_app::Supervisor;
use zigbee_plug_controller::{ButtonGesture, ButtonGestureEvent, NetworkStatus, status_led_on};

const SERVICE_PERIOD_MS: u32 = 10;
const REQUEST_NONE: u8 = 0;
const REQUEST_OFF: u8 = 1;
const REQUEST_ON: u8 = 2;
const METER_FAULT_NONE: u8 = 0;
const METER_FAULT_STARTUP_TIMEOUT: u8 = 1;
const METER_FAULT_STALE: u8 = 2;
const ISR_INHIBIT_METER_TIMEOUT: u8 = 1 << 0;
const ISR_INHIBIT_FACTORY_RESET: u8 = 1 << 1;

struct LocalState {
    relay: RelayOutput,
    led: StatusLed,
    button: UserButton,
    gesture: ButtonGesture,
    last_service_ms: u32,
    desired_relay_on: bool,
    applied_relay_on: bool,
    applied_led_on: bool,
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static REQUESTED_RELAY: AtomicU8 = AtomicU8::new(REQUEST_NONE);
static SAFETY_TRIPPED: AtomicBool = AtomicBool::new(false);
static RELAY_INHIBITED: AtomicBool = AtomicBool::new(true);
static NETWORK_STATUS: AtomicU8 = AtomicU8::new(NetworkStatus::Offline as u8);
static LOCAL_RELAY_ON: AtomicBool = AtomicBool::new(false);
static LOCAL_RELAY_SEQUENCE: AtomicU32 = AtomicU32::new(0);
static ACKED_LOCAL_RELAY_SEQUENCE: AtomicU32 = AtomicU32::new(0);
static FACTORY_RESET_REQUESTED: AtomicBool = AtomicBool::new(false);
static CLEAR_TRIP_REQUESTED: AtomicBool = AtomicBool::new(false);
static METER_TIMEOUT_ARMED: AtomicBool = AtomicBool::new(false);
static METER_TIMEOUT_DEADLINE_MS: AtomicU32 = AtomicU32::new(0);
static METER_TIMEOUT_KIND: AtomicU8 = AtomicU8::new(METER_FAULT_NONE);
static METER_TIMEOUT_REQUESTED: AtomicU8 = AtomicU8::new(METER_FAULT_NONE);
static ISR_INHIBIT_FLAGS: AtomicU8 = AtomicU8::new(0);
static mut LOCAL_STATE: MaybeUninit<LocalState> = MaybeUninit::uninit();

/// Consume the fixed GPIO resources after the shared app restored its state.
pub fn init_local_control(
    mut relay: RelayOutput,
    mut led: StatusLed,
    button: UserButton,
    initial_relay_on: bool,
    now_ms: u32,
) {
    relay.set_energized(initial_relay_on);
    led.set(false);
    let state = LocalState {
        relay,
        led,
        button,
        gesture: ButtonGesture::new(
            plug_efr32_proof::policy::BUTTON_DEBOUNCE_MS,
            plug_efr32_proof::policy::FACTORY_RESET_HOLD_MS,
        ),
        last_service_ms: now_ms,
        desired_relay_on: initial_relay_on,
        applied_relay_on: initial_relay_on,
        applied_led_on: false,
    };

    // SAFETY: single-shot startup writes the state before publishing it to
    // SysTick with a Release store.
    unsafe {
        core::ptr::addr_of_mut!(LOCAL_STATE).write(MaybeUninit::new(state));
    }
    REQUESTED_RELAY.store(REQUEST_NONE, Ordering::Release);
    SAFETY_TRIPPED.store(false, Ordering::Release);
    RELAY_INHIBITED.store(true, Ordering::Release);
    NETWORK_STATUS.store(NetworkStatus::Offline as u8, Ordering::Release);
    LOCAL_RELAY_ON.store(initial_relay_on, Ordering::Release);
    LOCAL_RELAY_SEQUENCE.store(0, Ordering::Release);
    ACKED_LOCAL_RELAY_SEQUENCE.store(0, Ordering::Release);
    FACTORY_RESET_REQUESTED.store(false, Ordering::Release);
    CLEAR_TRIP_REQUESTED.store(false, Ordering::Release);
    METER_TIMEOUT_ARMED.store(false, Ordering::Release);
    METER_TIMEOUT_KIND.store(METER_FAULT_NONE, Ordering::Release);
    METER_TIMEOUT_REQUESTED.store(METER_FAULT_NONE, Ordering::Release);
    ISR_INHIBIT_FLAGS.store(0, Ordering::Release);
    INITIALIZED.store(true, Ordering::Release);
}

/// Called by the 1 kHz SysTick handler; GPIO work is bounded to every 10 ms.
pub fn systick_1ms(now_ms: u32) {
    if !INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // SAFETY: only SysTick mutates this state after INITIALIZED is published.
    let state = unsafe { &mut *core::ptr::addr_of_mut!(LOCAL_STATE).cast::<LocalState>() };
    if now_ms.wrapping_sub(state.last_service_ms) < SERVICE_PERIOD_MS {
        return;
    }
    state.last_service_ms = now_ms;

    let requested = REQUESTED_RELAY.swap(REQUEST_NONE, Ordering::AcqRel);
    let local_change_pending = LOCAL_RELAY_SEQUENCE.load(Ordering::Acquire)
        != ACKED_LOCAL_RELAY_SEQUENCE.load(Ordering::Acquire);
    if !local_change_pending {
        match requested {
            REQUEST_OFF => state.desired_relay_on = false,
            REQUEST_ON => state.desired_relay_on = true,
            _ => {}
        }
    }

    if METER_TIMEOUT_ARMED.load(Ordering::Acquire) {
        let deadline_ms = METER_TIMEOUT_DEADLINE_MS.load(Ordering::Acquire);
        if now_ms.wrapping_sub(deadline_ms) < 0x8000_0000 {
            METER_TIMEOUT_ARMED.store(false, Ordering::Release);
            let fault = METER_TIMEOUT_KIND.load(Ordering::Acquire);
            METER_TIMEOUT_REQUESTED.store(fault, Ordering::Release);
            let flags = ISR_INHIBIT_FLAGS.load(Ordering::Acquire);
            ISR_INHIBIT_FLAGS.store(flags | ISR_INHIBIT_METER_TIMEOUT, Ordering::Release);
            SAFETY_TRIPPED.store(true, Ordering::Release);
            RELAY_INHIBITED.store(true, Ordering::Release);
            NETWORK_STATUS.store(NetworkStatus::Fault as u8, Ordering::Release);
            state.desired_relay_on = false;
            state.relay.set_energized(false);
            state.applied_relay_on = false;
        }
    }

    let isr_inhibited = ISR_INHIBIT_FLAGS.load(Ordering::Acquire) != 0;
    let safety_tripped = SAFETY_TRIPPED.load(Ordering::Acquire) || isr_inhibited;
    let relay_inhibited = RELAY_INHIBITED.load(Ordering::Acquire) || isr_inhibited;
    if safety_tripped || relay_inhibited {
        state.desired_relay_on = false;
    }

    match state.gesture.sample(now_ms, state.button.is_pressed()) {
        Some(ButtonGestureEvent::ShortPress) if safety_tripped => {
            CLEAR_TRIP_REQUESTED.store(true, Ordering::Release);
        }
        Some(ButtonGestureEvent::ShortPress) => {
            state.desired_relay_on = !state.desired_relay_on;
            LOCAL_RELAY_ON.store(state.desired_relay_on, Ordering::Release);
            LOCAL_RELAY_SEQUENCE.fetch_add(1, Ordering::AcqRel);
        }
        Some(ButtonGestureEvent::LongPress) => {
            let flags = ISR_INHIBIT_FLAGS.load(Ordering::Acquire);
            ISR_INHIBIT_FLAGS.store(flags | ISR_INHIBIT_FACTORY_RESET, Ordering::Release);
            SAFETY_TRIPPED.store(true, Ordering::Release);
            RELAY_INHIBITED.store(true, Ordering::Release);
            state.desired_relay_on = false;
            LOCAL_RELAY_ON.store(false, Ordering::Release);
            state.relay.set_energized(false);
            state.applied_relay_on = false;
            FACTORY_RESET_REQUESTED.store(true, Ordering::Release);
            NETWORK_STATUS.store(NetworkStatus::Searching as u8, Ordering::Release);
        }
        None => {}
    }

    let relay_on = state.desired_relay_on && !safety_tripped && !relay_inhibited;
    if relay_on != state.applied_relay_on {
        state.relay.set_energized(relay_on);
        state.applied_relay_on = relay_on;
    }

    let status = NetworkStatus::from_u8(NETWORK_STATUS.load(Ordering::Acquire));
    let led_on = status_led_on(status, relay_on, now_ms);
    if led_on != state.applied_led_on {
        state.led.set(led_on);
        state.applied_led_on = led_on;
    }
}

fn request_relay(on: bool) {
    REQUESTED_RELAY.store(if on { REQUEST_ON } else { REQUEST_OFF }, Ordering::Release);
}

fn take_flag(flag: &AtomicBool) -> bool {
    cortex_m::interrupt::free(|_| flag.swap(false, Ordering::AcqRel))
}

fn force_output_off_sync() {
    REQUESTED_RELAY.store(REQUEST_OFF, Ordering::Release);
    LOCAL_RELAY_ON.store(false, Ordering::Release);
    cortex_m::interrupt::free(|_| {
        if INITIALIZED.load(Ordering::Acquire) {
            // SAFETY: interrupts are masked, excluding the sole SysTick writer.
            let state = unsafe { &mut *core::ptr::addr_of_mut!(LOCAL_STATE).cast::<LocalState>() };
            state.desired_relay_on = false;
            state.relay.set_energized(false);
            state.applied_relay_on = false;
        }
    });
}

/// Synchronously force the fitted proof output inactive and show fault.
pub fn enter_fault() -> ! {
    SAFETY_TRIPPED.store(true, Ordering::Release);
    NETWORK_STATUS.store(NetworkStatus::Fault as u8, Ordering::Release);
    force_output_off_sync();
    cortex_m::interrupt::free(|_| {
        if INITIALIZED.load(Ordering::Acquire) {
            // SAFETY: interrupts are masked, excluding the sole SysTick writer.
            let state = unsafe { &mut *core::ptr::addr_of_mut!(LOCAL_STATE).cast::<LocalState>() };
            state.led.set(true);
            state.applied_led_on = true;
        }
    });
    loop {
        cortex_m::asm::wfi();
    }
}

/// Reset is the only safe terminal action before local GPIO ownership exists.
pub fn reset() -> ! {
    cortex_m::peripheral::SCB::sys_reset()
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EfrLocalControl;

impl LocalControl for EfrLocalControl {
    fn set_network_status(&mut self, status: NetworkStatus) {
        NETWORK_STATUS.store(status as u8, Ordering::Release);
    }

    fn take_local_selection(&mut self, last_sequence: &mut u32) -> Option<LocalRelaySelection> {
        let sequence = LOCAL_RELAY_SEQUENCE.load(Ordering::Acquire);
        if sequence == *last_sequence {
            return None;
        }
        *last_sequence = sequence;
        Some(LocalRelaySelection {
            sequence,
            relay_on: LOCAL_RELAY_ON.load(Ordering::Acquire),
        })
    }

    fn acknowledge_local_selection(&mut self, sequence: u32) {
        cortex_m::interrupt::free(|_| {
            if LOCAL_RELAY_SEQUENCE.load(Ordering::Acquire) == sequence {
                ACKED_LOCAL_RELAY_SEQUENCE.store(sequence, Ordering::Release);
            }
        });
    }

    fn take_clear_trip_requested(&mut self) -> bool {
        take_flag(&CLEAR_TRIP_REQUESTED)
    }

    fn take_factory_reset_requested(&mut self) -> bool {
        take_flag(&FACTORY_RESET_REQUESTED)
    }

    fn take_meter_timeout(&mut self) -> Option<MeterFault> {
        match cortex_m::interrupt::free(|_| {
            METER_TIMEOUT_REQUESTED.swap(METER_FAULT_NONE, Ordering::AcqRel)
        }) {
            METER_FAULT_STARTUP_TIMEOUT => Some(MeterFault::StartupTimeout),
            METER_FAULT_STALE => Some(MeterFault::Stale),
            _ => None,
        }
    }

    fn arm_meter_timeout(&mut self, timeout_ms: u32, fault: MeterFault) {
        let kind = match fault {
            MeterFault::StartupTimeout => METER_FAULT_STARTUP_TIMEOUT,
            MeterFault::Stale => METER_FAULT_STALE,
            MeterFault::InputDropped | MeterFault::InterfaceReset => return,
        };
        let now_ms = embassy_time::Instant::now().as_millis() as u32;
        METER_TIMEOUT_ARMED.store(false, Ordering::Release);
        METER_TIMEOUT_DEADLINE_MS.store(now_ms.wrapping_add(timeout_ms), Ordering::Release);
        METER_TIMEOUT_KIND.store(kind, Ordering::Release);
        METER_TIMEOUT_ARMED.store(true, Ordering::Release);
    }

    fn disarm_meter_timeout(&mut self) {
        METER_TIMEOUT_ARMED.store(false, Ordering::Release);
    }

    fn release_meter_timeout_inhibit(&mut self) {
        cortex_m::interrupt::free(|_| {
            let flags = ISR_INHIBIT_FLAGS.load(Ordering::Acquire);
            ISR_INHIBIT_FLAGS.store(flags & !ISR_INHIBIT_METER_TIMEOUT, Ordering::Release);
        });
    }

    fn release_factory_reset_inhibit(&mut self) {
        cortex_m::interrupt::free(|_| {
            let flags = ISR_INHIBIT_FLAGS.load(Ordering::Acquire);
            ISR_INHIBIT_FLAGS.store(flags & !ISR_INHIBIT_FACTORY_RESET, Ordering::Release);
        });
    }

    fn apply_relay(&mut self, command: RelayCommand) {
        SAFETY_TRIPPED.store(command.safety_tripped, Ordering::Release);
        RELAY_INHIBITED.store(command.relay_inhibited, Ordering::Release);
        request_relay(command.relay_on);
    }

    fn force_relay_off_sync(&mut self) {
        force_output_off_sync();
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EfrPlugClock;

impl PlugClock for EfrPlugClock {
    fn now_ms(&mut self) -> u32 {
        embassy_time::Instant::now().as_millis() as u32
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct EfrSupervisor;

impl Supervisor for EfrSupervisor {
    fn heartbeat(&mut self) {
        // No watchdog is claimed by this proof.
    }

    fn max_wait_ms(&self) -> Option<u32> {
        None
    }

    fn reset(&mut self) -> ! {
        reset()
    }
}
