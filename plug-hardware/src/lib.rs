//! Shared hardware metadata types and product flash-layout catalogs.
//!
//! Concrete fitted pin maps live in board crates. This crate performs no
//! register access and contains no board-specific GPIO constants.

#![no_std]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Port {
    A,
    B,
    C,
    D,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub port: Port,
    pub bit: u8,
}

impl Pin {
    pub const fn new(port: Port, bit: u8) -> Self {
        assert!(bit < 8);
        Self { port, bit }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveLevel {
    Low,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Output {
    pub pin: Pin,
    pub active: ActiveLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Input {
    pub pin: Pin,
    pub active: ActiveLevel,
    pub pull_up_ohms: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeteringPins {
    Bl0937 {
        cf: Pin,
        cf1: Pin,
        sel: Output,
    },
    Bl0942 {
        uart_tx: Pin,
        uart_rx: Pin,
        baud: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoardProfile {
    pub name: &'static str,
    pub relay: Output,
    pub leds: &'static [Output],
    pub button: Input,
    pub metering: MeteringPins,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashRegion {
    pub start: u32,
    pub end: u32,
}

impl FlashRegion {
    pub const fn new(start: u32, end: u32) -> Self {
        assert!(start < end);
        Self { start, end }
    }

    pub const fn size(self) -> u32 {
        self.end - self.start
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlashLayout {
    pub capacity: u32,
    pub firmware: FlashRegion,
    pub child_table_journal: FlashRegion,
    pub application_nv: FlashRegion,
    pub security_journal: FlashRegion,
    pub candidate_energy_journal: Option<FlashRegion>,
    pub factory_data: FlashRegion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    RegionOverlap,
    OutsideFlash,
}

impl FlashLayout {
    pub const fn validate(self) -> Result<(), LayoutError> {
        if self.factory_data.end > self.capacity {
            return Err(LayoutError::OutsideFlash);
        }
        if self.firmware.end > self.child_table_journal.start
            || self.child_table_journal.end > self.application_nv.start
            || self.application_nv.end > self.security_journal.start
            || self.security_journal.end > self.factory_data.start
        {
            return Err(LayoutError::RegionOverlap);
        }
        let energy_overlaps = match self.candidate_energy_journal {
            Some(energy) => {
                energy.start < self.security_journal.end || energy.end > self.factory_data.start
            }
            None => false,
        };
        if energy_overlaps {
            return Err(LayoutError::RegionOverlap);
        }
        Ok(())
    }
}

pub const TLSR8258_512K_LAYOUT: FlashLayout = FlashLayout {
    capacity: 0x0008_0000,
    firmware: FlashRegion::new(0x0000_0000, 0x0007_0000),
    child_table_journal: FlashRegion::new(0x0007_0000, 0x0007_2000),
    application_nv: FlashRegion::new(0x0007_2000, 0x0007_4000),
    security_journal: FlashRegion::new(0x0007_4000, 0x0007_6000),
    candidate_energy_journal: None,
    factory_data: FlashRegion::new(0x0007_6000, 0x0007_8000),
};

pub const TLSR8258_1M_LAYOUT: FlashLayout = FlashLayout {
    capacity: 0x0010_0000,
    firmware: FlashRegion::new(0x0000_0000, 0x0007_0000),
    child_table_journal: FlashRegion::new(0x0007_0000, 0x0007_2000),
    application_nv: FlashRegion::new(0x0007_2000, 0x0007_4000),
    security_journal: FlashRegion::new(0x0007_4000, 0x0007_6000),
    candidate_energy_journal: Some(FlashRegion::new(0x0009_6000, 0x000F_C000)),
    factory_data: FlashRegion::new(0x000F_E000, 0x0010_0000),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StockOtaFingerprint {
    pub manufacturer_code: u16,
    pub image_type: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    Documented,
    Experimental,
    PinMapOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductProfile {
    pub slug: &'static str,
    pub stock_manufacturer: Option<&'static str>,
    pub stock_model: &'static str,
    pub flash: FlashLayout,
    pub stock_ota: Option<StockOtaFingerprint>,
    pub evidence: Evidence,
}

impl ProductProfile {
    pub const fn validate(self) -> Result<(), LayoutError> {
        self.flash.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_layouts_are_non_overlapping() {
        assert_eq!(TLSR8258_512K_LAYOUT.validate(), Ok(()));
        assert_eq!(TLSR8258_1M_LAYOUT.validate(), Ok(()));
        assert_eq!(TLSR8258_512K_LAYOUT.child_table_journal.size(), 0x2000);
        assert_eq!(TLSR8258_1M_LAYOUT.child_table_journal.size(), 0x2000);
        assert_eq!(
            TLSR8258_512K_LAYOUT.child_table_journal.end,
            TLSR8258_512K_LAYOUT.application_nv.start
        );
        assert_eq!(
            TLSR8258_1M_LAYOUT.child_table_journal.end,
            TLSR8258_1M_LAYOUT.application_nv.start
        );
        assert_eq!(
            TLSR8258_1M_LAYOUT.candidate_energy_journal.unwrap().size(),
            0x66000
        );
    }
}
