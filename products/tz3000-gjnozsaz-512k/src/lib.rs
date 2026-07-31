#![no_std]

#[cfg(target_arch = "tc32")]
pub mod storage;

use zigbee_plug_hardware::{
    Evidence, ProductProfile, StockOtaFingerprint, TLSR8258_512K_LAYOUT, TS011F_BL0942_PC2,
};

pub const PRODUCT: ProductProfile = ProductProfile {
    slug: "tz3000-gjnozsaz-512k",
    stock_manufacturer: Some("_TZ3000_gjnozsaz"),
    stock_model: "TS011F",
    board: &TS011F_BL0942_PC2,
    flash: TLSR8258_512K_LAYOUT,
    stock_ota: Some(StockOtaFingerprint {
        manufacturer_code: 0x1286,
        image_type: 0x0002,
    }),
    evidence: Evidence::Experimental,
};

const _: () = assert!(PRODUCT.validate().is_ok());
