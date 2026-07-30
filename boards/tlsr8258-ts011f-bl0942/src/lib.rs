//! TLSR8258 TS011F board with BL0942 metering.

#![no_std]

pub use zigbee_plug_hardware::TS011F_BL0942_PC2 as PROFILE;

use tlsr8258_hal::gpio::Pin;

pub struct Bl0942Pins {
    pub tx: Pin,
    pub rx: Pin,
}

pub struct BoardResources {
    pub relay: Pin,
    pub led: Pin,
    pub button: Pin,
    pub metering: Bl0942Pins,
}

impl BoardResources {
    pub fn take() -> Option<Self> {
        let peripherals = tlsr8258_hal::peripherals::Peripherals::take()?;
        let tlsr8258_hal::peripherals::Pins {
            pb1,
            pb4,
            pb5,
            pb7,
            pc2,
            ..
        } = peripherals.pins;
        Some(Self {
            relay: pc2,
            led: pb4,
            button: pb5,
            metering: Bl0942Pins { tx: pb1, rx: pb7 },
        })
    }

    /// Configure the relay off before enabling its output driver.
    #[cfg(target_arch = "tc32")]
    pub fn initialize_safe(&self) -> Result<(), tlsr8258_hal::gpio::GpioError> {
        use tlsr8258_hal::gpio::{self, Pull};

        gpio::set_function_gpio(&self.relay);
        gpio::write(&self.relay, false);
        gpio::set_output_enable(&self.relay, true);

        gpio::set_function_gpio(&self.led);
        gpio::write(&self.led, true);
        gpio::set_output_enable(&self.led, true);

        gpio::set_function_gpio(&self.button);
        gpio::set_output_enable(&self.button, false);
        gpio::set_input_enable(&self.button, true)?;
        gpio::set_pull(&self.button, Pull::PullUp10K)?;
        Ok(())
    }

    #[cfg(target_arch = "tc32")]
    pub fn set_relay(&self, on: bool) {
        tlsr8258_hal::gpio::write(&self.relay, on);
    }

    #[cfg(target_arch = "tc32")]
    pub fn set_led(&self, on: bool) {
        tlsr8258_hal::gpio::write(&self.led, !on);
    }

    #[cfg(target_arch = "tc32")]
    pub fn button_pressed(&self) -> bool {
        !tlsr8258_hal::gpio::read(&self.button)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_matches_bl0942_board() {
        assert_eq!(PROFILE.name, "ts011f-bl0942-pc2");
        assert_eq!(PROFILE.leds.len(), 1);
    }
}
