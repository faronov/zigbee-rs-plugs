//! Explicit 1 MHz Embassy clock backed by a 1 kHz ARM SysTick interrupt.

use core::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};

use cortex_m::interrupt::Mutex;
use cortex_m_rt::exception;

const HCLK_HZ: u32 = efr32mg21_brd4181a_plug::HCLK_HZ;
const SYSTICK_RELOAD: u32 = HCLK_HZ / 1_000 - 1;
const TICKS_PER_MS: u64 = 1_000;

const SYST_CSR: *mut u32 = 0xE000_E010 as *mut u32;
const SYST_RVR: *mut u32 = 0xE000_E014 as *mut u32;
const SYST_CVR: *mut u32 = 0xE000_E018 as *mut u32;
const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
const CSR_ENABLE: u32 = 1 << 0;
const CSR_TICKINT: u32 = 1 << 1;
const CSR_CLKSOURCE: u32 = 1 << 2;
const ICSR_PENDSTSET: u32 = 1 << 26;

static MS_COUNT: AtomicU32 = AtomicU32::new(0);
static MS_EPOCH: AtomicU32 = AtomicU32::new(0);

struct AlarmState {
    target: u64,
    waker: Option<core::task::Waker>,
}

static ALARM: Mutex<RefCell<AlarmState>> = Mutex::new(RefCell::new(AlarmState {
    target: u64::MAX,
    waker: None,
}));

pub struct Efr32TimeDriver;

impl Efr32TimeDriver {
    pub const fn new() -> Self {
        Self
    }

    pub fn init(&self) {
        // SAFETY: startup is the sole SysTick configurator.
        unsafe {
            core::ptr::write_volatile(SYST_RVR, SYSTICK_RELOAD);
            core::ptr::write_volatile(SYST_CVR, 0);
            core::ptr::write_volatile(SYST_CSR, CSR_CLKSOURCE | CSR_TICKINT | CSR_ENABLE);
        }
    }
}

impl embassy_time_driver::Driver for Efr32TimeDriver {
    fn now(&self) -> u64 {
        cortex_m::interrupt::free(|_| {
            loop {
                let epoch = MS_EPOCH.load(Ordering::Relaxed) as u64;
                let ms = MS_COUNT.load(Ordering::Relaxed) as u64;
                // SAFETY: ICSR and CVR are standard read-only Cortex-M registers.
                let pending_before =
                    unsafe { core::ptr::read_volatile(SCB_ICSR) } & ICSR_PENDSTSET != 0;
                let remaining = unsafe { core::ptr::read_volatile(SYST_CVR.cast_const()) } as u64;
                let pending_after =
                    unsafe { core::ptr::read_volatile(SCB_ICSR) } & ICSR_PENDSTSET != 0;
                if pending_before != pending_after {
                    continue;
                }

                let full_ms = ((epoch << 32) | ms) + u64::from(pending_after);
                let elapsed = SYSTICK_RELOAD as u64 - remaining;
                return full_ms * TICKS_PER_MS + elapsed * 1_000_000 / HCLK_HZ as u64;
            }
        })
    }

    fn schedule_wake(&self, at: u64, waker: &core::task::Waker) {
        cortex_m::interrupt::free(|cs| {
            let mut alarm = ALARM.borrow(cs).borrow_mut();
            alarm.target = at;
            alarm.waker = Some(waker.clone());
        });

        if self.now() >= at {
            cortex_m::interrupt::free(|cs| {
                let mut alarm = ALARM.borrow(cs).borrow_mut();
                if alarm.target == at {
                    alarm.target = u64::MAX;
                    if let Some(waker) = alarm.waker.take() {
                        waker.wake();
                    }
                }
            });
        }
    }
}

#[exception]
fn SysTick() {
    let next = MS_COUNT.load(Ordering::Relaxed).wrapping_add(1);
    MS_COUNT.store(next, Ordering::Relaxed);
    if next == 0 {
        MS_EPOCH.fetch_add(1, Ordering::Relaxed);
    }

    let epoch = MS_EPOCH.load(Ordering::Relaxed) as u64;
    let full_ms = (epoch << 32) | next as u64;
    let now_ticks = full_ms * TICKS_PER_MS;

    crate::platform::systick_1ms(full_ms as u32);

    cortex_m::interrupt::free(|cs| {
        let mut alarm = ALARM.borrow(cs).borrow_mut();
        if now_ticks >= alarm.target {
            alarm.target = u64::MAX;
            if let Some(waker) = alarm.waker.take() {
                waker.wake();
            }
        }
    });
}

embassy_time_driver::time_driver_impl!(
    static TIME_DRIVER: Efr32TimeDriver = Efr32TimeDriver::new()
);

pub fn init() {
    TIME_DRIVER.init();
}
