//! Timer1-driven button, relay, and network-status LED service.
//!
//! Telink's MAC and BDB operations use bounded synchronous waits inside async
//! methods, so the application loop cannot poll GPIO while a channel scan or
//! association attempt is running. A short Timer1 ISR keeps the physical
//! control responsive without touching Zigbee/profile state from interrupt
//! context. The main loop later reconciles local relay changes into ZCL.

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use plug_router_app::MeterFault;
use tlsr8258_hal::gpio::Pin;
use tlsr8258_hal::timer;
use zigbee_plug_controller::{ButtonGesture, ButtonGestureEvent, NetworkStatus, status_led_on};
use zigbee_plug_core::TickMillis;

const TIMER_PERIOD_MS: u32 = 10;
const BUTTON_DEBOUNCE_MS: u32 = 30;
const FACTORY_RESET_HOLD_MS: u32 = 4_000;

const RELAY_REQUEST_NONE: u8 = 0;
const RELAY_REQUEST_OFF: u8 = 1;
const RELAY_REQUEST_ON: u8 = 2;
const METER_FAULT_NONE: u8 = 0;
const METER_FAULT_STARTUP_TIMEOUT: u8 = 1;
const METER_FAULT_STALE: u8 = 2;
const ISR_INHIBIT_METER_TIMEOUT: u8 = 1 << 0;
const ISR_INHIBIT_FACTORY_RESET: u8 = 1 << 1;

type SetOutput = fn(&Pin, bool);
type ReadButton = fn(&Pin) -> bool;

struct IsrState {
    relay: Pin,
    led: Pin,
    button: Pin,
    set_relay: SetOutput,
    set_led: SetOutput,
    read_button: ReadButton,
    gesture: ButtonGesture,
    clock: TickMillis,
    desired_relay_on: bool,
    applied_relay_on: bool,
    applied_led_on: bool,
}

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static REQUESTED_RELAY: AtomicU8 = AtomicU8::new(RELAY_REQUEST_NONE);
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
static mut ISR_STATE: MaybeUninit<IsrState> = MaybeUninit::uninit();

/// Consume the three GPIO ownership tokens and start the 10 ms service IRQ.
pub fn init(
    relay: Pin,
    led: Pin,
    button: Pin,
    set_relay: SetOutput,
    set_led: SetOutput,
    read_button: ReadButton,
    initial_relay_on: bool,
) {
    timer::start_timer1();
    let initial_ticks = timer::now_ticks1();
    let state = IsrState {
        relay,
        led,
        button,
        set_relay,
        set_led,
        read_button,
        gesture: ButtonGesture::new(BUTTON_DEBOUNCE_MS, FACTORY_RESET_HOLD_MS),
        clock: TickMillis::new(timer::TICKS_PER_MS, initial_ticks)
            .expect("Timer1 has a nonzero tick rate"),
        desired_relay_on: initial_relay_on,
        applied_relay_on: initial_relay_on,
        applied_led_on: false,
    };
    (state.set_relay)(&state.relay, initial_relay_on);
    (state.set_led)(&state.led, false);

    // SAFETY: initialization is single-shot and the IRQ remains masked until
    // the state is fully written and `INITIALIZED` is published.
    unsafe {
        core::ptr::addr_of_mut!(ISR_STATE).write(MaybeUninit::new(state));
    }
    LOCAL_RELAY_ON.store(initial_relay_on, Ordering::Release);
    METER_TIMEOUT_ARMED.store(false, Ordering::Release);
    METER_TIMEOUT_KIND.store(METER_FAULT_NONE, Ordering::Release);
    METER_TIMEOUT_REQUESTED.store(METER_FAULT_NONE, Ordering::Release);
    ISR_INHIBIT_FLAGS.store(0, Ordering::Release);

    timer::clear_timer1_irq_pending();
    let next = timer::now_ticks1().wrapping_add(timer::ms(TIMER_PERIOD_MS));
    timer::set_timer1_capture_ticks(next);
    INITIALIZED.store(true, Ordering::Release);
    timer::set_timer1_irq_enable(true);
}

/// Service Timer1 from the shared TLSR8258 interrupt vector.
pub fn handle_timer_irq() {
    if !timer::timer1_irq_pending() {
        return;
    }
    timer::clear_timer1_irq_pending();
    let now_ticks = timer::now_ticks1();
    timer::set_timer1_capture_ticks(now_ticks.wrapping_add(timer::ms(TIMER_PERIOD_MS)));

    if !INITIALIZED.load(Ordering::Acquire) {
        return;
    }

    // SAFETY: only this IRQ mutates the state while interrupts are enabled.
    let state = unsafe { &mut *core::ptr::addr_of_mut!(ISR_STATE).cast::<IsrState>() };
    let requested = REQUESTED_RELAY.load(Ordering::Acquire);
    REQUESTED_RELAY.store(RELAY_REQUEST_NONE, Ordering::Release);
    let local_change_pending = LOCAL_RELAY_SEQUENCE.load(Ordering::Acquire)
        != ACKED_LOCAL_RELAY_SEQUENCE.load(Ordering::Acquire);
    if !local_change_pending {
        match requested {
            RELAY_REQUEST_OFF => state.desired_relay_on = false,
            RELAY_REQUEST_ON => state.desired_relay_on = true,
            _ => {}
        }
    }

    let now_ms = state.clock.update(now_ticks);
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
            (state.set_relay)(&state.relay, false);
            state.applied_relay_on = false;
        }
    }

    let isr_inhibited = ISR_INHIBIT_FLAGS.load(Ordering::Acquire) != 0;
    let safety_tripped = SAFETY_TRIPPED.load(Ordering::Acquire) || isr_inhibited;
    let relay_inhibited = RELAY_INHIBITED.load(Ordering::Acquire) || isr_inhibited;
    if safety_tripped || relay_inhibited {
        state.desired_relay_on = false;
    }

    let pressed = (state.read_button)(&state.button);
    match state.gesture.sample(now_ms, pressed) {
        Some(ButtonGestureEvent::ShortPress) if safety_tripped => {
            CLEAR_TRIP_REQUESTED.store(true, Ordering::Release);
        }
        Some(ButtonGestureEvent::ShortPress) => {
            state.desired_relay_on = !state.desired_relay_on;
            LOCAL_RELAY_ON.store(state.desired_relay_on, Ordering::Release);
            let sequence = LOCAL_RELAY_SEQUENCE.load(Ordering::Acquire);
            LOCAL_RELAY_SEQUENCE.store(sequence.wrapping_add(1), Ordering::Release);
        }
        Some(ButtonGestureEvent::LongPress) => {
            let flags = ISR_INHIBIT_FLAGS.load(Ordering::Acquire);
            ISR_INHIBIT_FLAGS.store(flags | ISR_INHIBIT_FACTORY_RESET, Ordering::Release);
            SAFETY_TRIPPED.store(true, Ordering::Release);
            RELAY_INHIBITED.store(true, Ordering::Release);
            state.desired_relay_on = false;
            LOCAL_RELAY_ON.store(false, Ordering::Release);
            (state.set_relay)(&state.relay, false);
            state.applied_relay_on = false;
            FACTORY_RESET_REQUESTED.store(true, Ordering::Release);
            NETWORK_STATUS.store(NetworkStatus::Searching as u8, Ordering::Release);
        }
        None => {}
    }

    let relay_on = state.desired_relay_on && !safety_tripped && !relay_inhibited;
    if relay_on != state.applied_relay_on {
        (state.set_relay)(&state.relay, relay_on);
        state.applied_relay_on = relay_on;
    }

    let status = NetworkStatus::from_u8(NETWORK_STATUS.load(Ordering::Acquire));
    let led_on = status_led_on(status, relay_on, now_ms);
    if led_on != state.applied_led_on {
        (state.set_led)(&state.led, led_on);
        state.applied_led_on = led_on;
    }
}

pub fn request_relay(on: bool) {
    REQUESTED_RELAY.store(
        if on {
            RELAY_REQUEST_ON
        } else {
            RELAY_REQUEST_OFF
        },
        Ordering::Release,
    );
}

pub fn set_safety_tripped(tripped: bool) {
    SAFETY_TRIPPED.store(tripped, Ordering::Release);
}

pub fn set_relay_inhibited(inhibited: bool) {
    RELAY_INHIBITED.store(inhibited, Ordering::Release);
}

pub fn set_network_status(status: NetworkStatus) {
    NETWORK_STATUS.store(status as u8, Ordering::Release);
}

pub fn take_factory_reset_requested() -> bool {
    tlsr8258_hal::mmio::with_irqs_disabled(|| {
        let requested = FACTORY_RESET_REQUESTED.load(Ordering::Acquire);
        FACTORY_RESET_REQUESTED.store(false, Ordering::Release);
        requested
    })
}

pub fn take_clear_trip_requested() -> bool {
    tlsr8258_hal::mmio::with_irqs_disabled(|| {
        let requested = CLEAR_TRIP_REQUESTED.load(Ordering::Acquire);
        CLEAR_TRIP_REQUESTED.store(false, Ordering::Release);
        requested
    })
}

pub fn arm_meter_timeout(timeout_ms: u32, fault: MeterFault) {
    let kind = match fault {
        MeterFault::StartupTimeout => METER_FAULT_STARTUP_TIMEOUT,
        MeterFault::Stale => METER_FAULT_STALE,
        MeterFault::InputDropped | MeterFault::InterfaceReset => return,
    };
    tlsr8258_hal::mmio::with_irqs_disabled(|| {
        if !INITIALIZED.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: Timer1 is masked while its private clock is advanced.
        let state = unsafe { &mut *core::ptr::addr_of_mut!(ISR_STATE).cast::<IsrState>() };
        let now_ms = state.clock.update(timer::now_ticks1());
        METER_TIMEOUT_ARMED.store(false, Ordering::Release);
        METER_TIMEOUT_DEADLINE_MS.store(now_ms.wrapping_add(timeout_ms), Ordering::Release);
        METER_TIMEOUT_KIND.store(kind, Ordering::Release);
        METER_TIMEOUT_ARMED.store(true, Ordering::Release);
    });
}

pub fn disarm_meter_timeout() {
    METER_TIMEOUT_ARMED.store(false, Ordering::Release);
}

pub fn take_meter_timeout() -> Option<MeterFault> {
    tlsr8258_hal::mmio::with_irqs_disabled(|| {
        let requested = METER_TIMEOUT_REQUESTED.load(Ordering::Acquire);
        METER_TIMEOUT_REQUESTED.store(METER_FAULT_NONE, Ordering::Release);
        match requested {
            METER_FAULT_STARTUP_TIMEOUT => Some(MeterFault::StartupTimeout),
            METER_FAULT_STALE => Some(MeterFault::Stale),
            _ => None,
        }
    })
}

fn release_isr_inhibit(flag: u8) {
    tlsr8258_hal::mmio::with_irqs_disabled(|| {
        let flags = ISR_INHIBIT_FLAGS.load(Ordering::Acquire);
        ISR_INHIBIT_FLAGS.store(flags & !flag, Ordering::Release);
    });
}

pub fn release_meter_timeout_inhibit() {
    release_isr_inhibit(ISR_INHIBIT_METER_TIMEOUT);
}

pub fn release_factory_reset_inhibit() {
    release_isr_inhibit(ISR_INHIBIT_FACTORY_RESET);
}

/// Return the latest locally selected relay state once for each button change.
pub fn take_local_relay_change(last_sequence: &mut u32) -> Option<bool> {
    let sequence = LOCAL_RELAY_SEQUENCE.load(Ordering::Acquire);
    if sequence == *last_sequence {
        return None;
    }
    *last_sequence = sequence;
    Some(LOCAL_RELAY_ON.load(Ordering::Acquire))
}

/// Acknowledge a local selection only after it has been copied into ZCL.
pub fn acknowledge_local_relay_change(sequence: u32) {
    tlsr8258_hal::mmio::with_irqs_disabled(|| {
        if LOCAL_RELAY_SEQUENCE.load(Ordering::Acquire) == sequence {
            ACKED_LOCAL_RELAY_SEQUENCE.store(sequence, Ordering::Release);
        }
    });
}

pub fn force_relay_off_sync() {
    REQUESTED_RELAY.store(RELAY_REQUEST_OFF, Ordering::Release);
    LOCAL_RELAY_ON.store(false, Ordering::Release);
    tlsr8258_hal::mmio::with_irqs_disabled(|| {
        if !INITIALIZED.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: all IRQs are masked for this synchronous relay write.
        let state = unsafe { &mut *core::ptr::addr_of_mut!(ISR_STATE).cast::<IsrState>() };
        state.desired_relay_on = false;
        (state.set_relay)(&state.relay, false);
        state.applied_relay_on = false;
    });
}

pub fn enter_fault() -> ! {
    set_safety_tripped(true);
    set_network_status(NetworkStatus::Fault);
    force_relay_off_sync();
    tlsr8258_hal::mmio::with_irqs_disabled(|| {
        if !INITIALIZED.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: all IRQs are masked for this synchronous fault indication.
        let state = unsafe { &mut *core::ptr::addr_of_mut!(ISR_STATE).cast::<IsrState>() };
        (state.set_led)(&state.led, true);
        state.applied_led_on = true;
    });
    loop {
        core::hint::spin_loop();
    }
}
