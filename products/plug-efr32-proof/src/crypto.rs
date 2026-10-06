//! Explicit AES provider gate for the EFR32 proof.
//!
//! `Efr32s2Mac` currently keeps `ForwardAesProvider`'s RustCrypto software
//! implementation. The startup KAT below proves that selected provider's
//! AES-128 forward primitive; it does not claim use or validation of the
//! EFR32 Secure Element/CRYPTO hardware.

use zigbee_crypto::{Aes128Forward, ForwardAesProvider};

pub const AES_IMPLEMENTATION: &str = "RustCrypto software AES-128";
pub const HARDWARE_AES: bool = false;

pub fn aes_startup_known_answer_test<P: ForwardAesProvider>(provider: &mut P) -> bool {
    let key = [
        0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
        0x0F,
    ];
    let mut block = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
        0xFF,
    ];
    let expected = [
        0x69, 0xC4, 0xE0, 0xD8, 0x6A, 0x7B, 0x04, 0x30, 0xD8, 0xCD, 0xB7, 0x80, 0x70, 0xB4, 0xC5,
        0x5A,
    ];

    let mut cipher = provider.forward_cipher(&key);
    cipher.encrypt_block(&mut block).is_ok() && block == expected
}

#[cfg(test)]
mod tests {
    use super::*;
    use zigbee_crypto::SoftwareAesProvider;

    #[test]
    fn selected_development_provider_passes_fips_197_vector() {
        let mut provider = SoftwareAesProvider::new();
        assert!(aes_startup_known_answer_test(&mut provider));
        assert_eq!(AES_IMPLEMENTATION, "RustCrypto software AES-128");
        assert!(!core::hint::black_box(HARDWARE_AES));
    }
}
