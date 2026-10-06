#![no_std]

#[cfg(target_arch = "tc32")]
pub mod storage;

use zigbee_plug_hardware::{Evidence, ProductProfile, StockOtaFingerprint, TLSR8258_1M_LAYOUT};

pub const PRODUCT: ProductProfile = ProductProfile {
    slug: "tz3000-gjnozsaz-1m",
    stock_manufacturer: Some("_TZ3000_gjnozsaz"),
    stock_model: "TS011F",
    flash: TLSR8258_1M_LAYOUT,
    stock_ota: Some(StockOtaFingerprint {
        manufacturer_code: 0x1141,
        image_type: 0xD3A3,
    }),
    evidence: Evidence::Documented,
};

const _: () = assert!(PRODUCT.validate().is_ok());
