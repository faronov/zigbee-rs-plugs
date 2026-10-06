//! Development-only EFR32MG21 smart-plug proof product.
//!
//! The shared `ZigbeePlug`, `PlugController`, and `plug-router-app` behavior
//! are unchanged. Because `Efr32s2Mac` does not implement `ParentMacDriver`,
//! this product selects a conformant receiver-on End Device role rather than
//! advertising unsupported Router capability. It allocates no child journal.
//!
//! This is not a mains-qualified or metering-qualified product.

#![no_std]

pub mod crypto;
pub mod meter;
pub mod policy;
pub mod profile;
pub mod storage;

pub const MANUFACTURER: &str = "Zigbee-RS";
pub const MODEL: &str = "EFR32MG21-RelayPlug";
pub const DATE_CODE: &str = "20260826";
pub const SW_BUILD: &str = "0.1.0-proof";
pub const APPLICATION_VERSION: u8 = 1;
pub const ENDPOINT: u8 = 1;

/// Compile-time product contract: this image never admits children.
pub const CHILD_ADMISSION: bool = false;
