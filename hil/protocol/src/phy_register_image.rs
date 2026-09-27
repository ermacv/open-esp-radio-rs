//! Read-only observation of the production radio-PHY register image.
//!
//! The image is the published `RadioPhyPeripherals` partition's readable
//! registers in the register model's order; a request names a window of
//! indices and the reply carries their values. Hardware cross-checks compare
//! it with the vendor firmware's register state after calibration.

use serde::{Deserialize, Serialize};

/// Most register words one reply carries.
pub const PHY_REGISTER_IMAGE_WORDS: usize = 16;

/// Read `count` registers of the image from index `first`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyRegisterImageRequest {
    pub first: u16,
    pub count: u8,
}

/// The values of a window of the image.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyRegisterImageWords {
    pub first: u16,
    /// Registers of the whole image.
    pub length: u16,
    pub values: heapless::Vec<u32, PHY_REGISTER_IMAGE_WORDS>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, Event};

    #[test]
    fn a_full_window_survives_the_wire_encoding() {
        let words = PhyRegisterImageWords {
            first: 32,
            length: 209,
            values: (0..PHY_REGISTER_IMAGE_WORDS as u32).collect(),
        };
        let mut buffer = [0; 256];
        let encoded =
            postcard::to_slice(&Event::PhyRegisterImage(words.clone()), &mut buffer).unwrap();
        let decoded: Event = postcard::from_bytes(encoded).unwrap();
        assert_eq!(decoded, Event::PhyRegisterImage(words));
        let request = Command::PhyRegisterImage(PhyRegisterImageRequest {
            first: 0,
            count: 16,
        });
        let encoded = postcard::to_slice(&request, &mut buffer).unwrap();
        assert_eq!(postcard::from_bytes::<Command>(encoded).unwrap(), request);
    }
}
