#![no_std]

#[cfg(target_arch = "tc32")]
pub mod storage;

use zigbee_plug_hardware::{Evidence, ProductProfile, TLSR8258_1M_LAYOUT};

pub const PRODUCT: ProductProfile = ProductProfile {
    slug: "legacy-bl0937-pd6",
    stock_manufacturer: None,
    stock_model: "TS011F",
    flash: TLSR8258_1M_LAYOUT,
    stock_ota: None,
    evidence: Evidence::PinMapOnly,
};

const _: () = assert!(PRODUCT.validate().is_ok());
