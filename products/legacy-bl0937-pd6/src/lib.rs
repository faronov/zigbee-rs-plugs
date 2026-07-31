#![no_std]

#[cfg(target_arch = "tc32")]
pub mod storage;

use zigbee_plug_hardware::{Evidence, LEGACY_BL0937_PD6, ProductProfile, TLSR8258_1M_LAYOUT};

pub const PRODUCT: ProductProfile = ProductProfile {
    slug: "legacy-bl0937-pd6",
    stock_manufacturer: None,
    stock_model: "TS011F",
    board: &LEGACY_BL0937_PD6,
    flash: TLSR8258_1M_LAYOUT,
    stock_ota: None,
    evidence: Evidence::PinMapOnly,
};

const _: () = assert!(PRODUCT.validate().is_ok());
