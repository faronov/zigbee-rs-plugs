//! Full 51-entry EFR32MG21 interrupt vector table.

#[repr(C)]
pub union Vector {
    handler: unsafe extern "C" fn(),
    reserved: usize,
}

unsafe impl Sync for Vector {}

#[allow(dead_code)]
unsafe extern "C" {
    fn EMU();
    fn SE();
    fn IADC();
    fn GPIO_EVEN();
    fn TIMER0();
    fn USART0_RX();
    fn USART0_TX();
    fn USART1_RX();
    fn USART1_TX();
    fn USART2_RX();
    fn USART2_TX();
    fn I2C0();
    fn I2C1();
    fn GPIO_ODD();
    fn LDMA();
    fn LDMA_CH0();
    fn LDMA_CH1();
    fn LDMA_CH2();
    fn LDMA_CH3();
    fn LDMA_CH4();
    fn LDMA_CH5();
    fn LDMA_CH6();
    fn LDMA_CH7();
    fn TIMER1();
    fn TIMER2();
    fn TIMER3();
    fn WDOG0();
    fn WDOG1();
    fn SYSCFG();
    fn MSC();
    fn CRYPTO();
    fn CMU();
    fn PRS_ASYNC_CH0();
    fn PRS_ASYNC_CH1();
    fn PRS_ASYNC_CH2();
    fn PRS_ASYNC_CH3();
    fn FRC_PRI();
    fn FRC();
    fn MODEM();
    fn PROTIMER();
    fn RAC_RSM();
    fn RAC_SEQ();
    fn RDMAILBOX();
    fn RFSENSE();
    fn PRORTC();
    fn SYNTH();
    fn BUFC();
    fn AGC();
    fn LETIMER0();
    fn BURTC();
    fn RTCC();
}

#[unsafe(link_section = ".vector_table.interrupts")]
#[unsafe(no_mangle)]
pub static __INTERRUPTS: [Vector; 51] = [
    Vector { handler: EMU },
    Vector { handler: SE },
    Vector { handler: IADC },
    Vector { handler: GPIO_EVEN },
    Vector { handler: TIMER0 },
    Vector { handler: USART0_RX },
    Vector { handler: USART0_TX },
    Vector { handler: USART1_RX },
    Vector { handler: USART1_TX },
    Vector { handler: USART2_RX },
    Vector { handler: USART2_TX },
    Vector { handler: I2C0 },
    Vector { handler: I2C1 },
    Vector { handler: GPIO_ODD },
    Vector { handler: LDMA },
    Vector { handler: LDMA_CH0 },
    Vector { handler: LDMA_CH1 },
    Vector { handler: LDMA_CH2 },
    Vector { handler: LDMA_CH3 },
    Vector { handler: LDMA_CH4 },
    Vector { handler: LDMA_CH5 },
    Vector { handler: LDMA_CH6 },
    Vector { handler: LDMA_CH7 },
    Vector { handler: TIMER1 },
    Vector { handler: TIMER2 },
    Vector { handler: TIMER3 },
    Vector { handler: WDOG0 },
    Vector { handler: WDOG1 },
    Vector { handler: SYSCFG },
    Vector { handler: MSC },
    Vector { handler: CRYPTO },
    Vector { handler: CMU },
    Vector {
        handler: PRS_ASYNC_CH0,
    },
    Vector {
        handler: PRS_ASYNC_CH1,
    },
    Vector {
        handler: PRS_ASYNC_CH2,
    },
    Vector {
        handler: PRS_ASYNC_CH3,
    },
    Vector { handler: FRC_PRI },
    Vector { handler: FRC },
    Vector { handler: MODEM },
    Vector { handler: PROTIMER },
    Vector { handler: RAC_RSM },
    Vector { handler: RAC_SEQ },
    Vector { handler: RDMAILBOX },
    Vector { handler: RFSENSE },
    Vector { handler: PRORTC },
    Vector { handler: SYNTH },
    Vector { handler: BUFC },
    Vector { handler: AGC },
    Vector { handler: LETIMER0 },
    Vector { handler: BURTC },
    Vector { handler: RTCC },
];

#[derive(Debug, Clone, Copy)]
#[repr(u16)]
#[allow(dead_code)]
pub enum Interrupt {
    FrcPri = 36,
}

unsafe impl cortex_m::interrupt::InterruptNumber for Interrupt {
    fn number(self) -> u16 {
        self as u16
    }
}
