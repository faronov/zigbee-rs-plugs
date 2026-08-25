//! Timer1-driven button, relay, and network-status LED service.
//!
//! Telink's MAC and BDB operations use bounded synchronous waits inside async
//! methods, so the application loop cannot poll GPIO while a channel scan or
//! association attempt is running. A short Timer1 ISR keeps the physical
//! control responsive without touching Zigbee/profile state from interrupt
//! context. The main loop later reconciles local relay changes into ZCL.

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use tlsr8258_hal::gpio::Pin;
use tlsr8258_hal::timer;
use zigbee_plug_controller::{
    ButtonGesture, ButtonGestureEvent, NetworkStatus, status_led_on,
};
use zigbee_plug_core::TickMillis;

const TIMER_PERIOD_MS: u32 = 10;
const BUTTON_DEBOUNCE_MS: u32 = 30;
const FACTORY_RESET_HOLD_MS: u32 = 4_000;

const RELAY_REQUEST_NONE: u8 = 0;
const RELAY_REQUEST_OFF: u8 = 1;
const RELAY_REQUEST_ON: u8 = 2;

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
static NETWORK_STATUS: AtomicU8 = AtomicU8::new(NetworkStatus::Offline as u8);
static LOCAL_RELAY_ON: AtomicBool = AtomicBool::new(false);
static LOCAL_RELAY_SEQUENCE: AtomicU32 = AtomicU32::new(0);
static ACKED_LOCAL_RELAY_SEQUENCE: AtomicU32 = AtomicU32::new(0);
static FACTORY_RESET_REQUESTED: AtomicBool = AtomicBool::new(false);
static CLEAR_TRIP_REQUESTED: AtomicBool = AtomicBool::new(false);
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

    let safety_tripped = SAFETY_TRIPPED.load(Ordering::Acquire);
    if safety_tripped {
        state.desired_relay_on = false;
    }

    let now_ms = state.clock.update(now_ticks);
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
            FACTORY_RESET_REQUESTED.store(true, Ordering::Release);
            NETWORK_STATUS.store(NetworkStatus::Searching as u8, Ordering::Release);
        }
        None => {}
    }

    let relay_on = state.desired_relay_on && !safety_tripped;
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

pub fn enter_fault() {
    set_safety_tripped(true);
    set_network_status(NetworkStatus::Fault);
    tlsr8258_hal::mmio::with_irqs_disabled(|| {
        if !INITIALIZED.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: all IRQs are masked for this synchronous fail-closed write.
        let state = unsafe { &mut *core::ptr::addr_of_mut!(ISR_STATE).cast::<IsrState>() };
        state.desired_relay_on = false;
        (state.set_relay)(&state.relay, false);
        state.applied_relay_on = false;
        (state.set_led)(&state.led, true);
        state.applied_led_on = true;
    });
}
