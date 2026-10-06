//! Exclusive raw onboard-flash ownership.

use efr32mg21_hal::flash::Efr32mg21Flash;

/// Exclusive onboard-flash token produced only by `BoardResources::take`.
///
/// Partition ranges and journal formats belong to the selected product.
pub struct StorageToken(Efr32mg21Flash);

impl StorageToken {
    pub(crate) const fn new(flash: Efr32mg21Flash) -> Self {
        Self(flash)
    }

    pub fn into_flash(self) -> Efr32mg21Flash {
        self.0
    }
}
