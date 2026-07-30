#![no_std]

use zigbee_plug_hardware::{
    Evidence, ProductProfile, StockOtaFingerprint, TLSR8258_1M_LAYOUT, TS011F_BL0942_PC2,
};

pub const PRODUCT: ProductProfile = ProductProfile {
    slug: "tz3000-gjnozsaz-1m",
    stock_manufacturer: Some("_TZ3000_gjnozsaz"),
    stock_model: "TS011F",
    board: &TS011F_BL0942_PC2,
    flash: TLSR8258_1M_LAYOUT,
    stock_ota: Some(StockOtaFingerprint {
        manufacturer_code: 0x1141,
        image_type: 0xD3A3,
    }),
    evidence: Evidence::Documented,
};

const _: () = assert!(PRODUCT.validate().is_ok());
