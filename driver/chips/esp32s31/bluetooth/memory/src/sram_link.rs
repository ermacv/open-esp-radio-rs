//! Nonzero compressed links for reviewed bound DTM header/link-state fields.

#![forbid(unsafe_code)]

use open_esp_radio_esp32s31_hal::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};

/// Why an address cannot represent a reviewed bound DTM link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothDtmBoundSramLinkAddressError {
    /// The positional address is outside the PAC compression domain.
    InvalidAddress(BluetoothControllerSramAddressError),
    /// The window base compresses to zero, which represents an unbound link in
    /// the reviewed DTM header and link-state contexts.
    ZeroCompressedImage,
}

/// A nonzero compressed link for reviewed bound DTM contexts.
///
/// Unlike the positional PAC address, this semantic memory-layer value cannot
/// collapse `Some(address)` onto the zero image used for `None`. It remains a
/// forgeable geometry value and is not a backing-storage or publication token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDtmBoundSramLinkAddress(BluetoothControllerSramAddress);

impl BluetoothDtmBoundSramLinkAddress {
    /// Validate an aligned address and reject the zero compressed image.
    pub const fn new(address: u32) -> Result<Self, BluetoothDtmBoundSramLinkAddressError> {
        let address = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => {
                return Err(BluetoothDtmBoundSramLinkAddressError::InvalidAddress(error));
            }
        };
        if address.compressed_image() == 0 {
            return Err(BluetoothDtmBoundSramLinkAddressError::ZeroCompressedImage);
        }
        Ok(Self(address))
    }

    /// Return the exact nonzero low-twenty-bit link image.
    pub const fn compressed_image(self) -> u32 {
        self.0.compressed_image()
    }
}

#[cfg(test)]
mod tests {
    use open_esp_radio_esp32s31_hal::BluetoothControllerSramAddressError;

    use super::{BluetoothDtmBoundSramLinkAddress, BluetoothDtmBoundSramLinkAddressError};

    #[test]
    fn bound_link_cannot_collapse_onto_the_zero_unbound_image() {
        assert_eq!(
            BluetoothDtmBoundSramLinkAddress::new(0x2f00_0000),
            Err(BluetoothDtmBoundSramLinkAddressError::ZeroCompressedImage)
        );
        assert_eq!(
            BluetoothDtmBoundSramLinkAddress::new(0x2f00_0004)
                .expect("first nonzero compressed link is valid")
                .compressed_image(),
            1
        );
        assert_eq!(
            BluetoothDtmBoundSramLinkAddress::new(0x2f00_0001),
            Err(BluetoothDtmBoundSramLinkAddressError::InvalidAddress(
                BluetoothControllerSramAddressError::Unaligned
            ))
        );
    }
}
