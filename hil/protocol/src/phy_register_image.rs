//! Observation of the production radio-PHY register and analog images.
//!
//! The register image is the published `RadioPhyPeripherals` partition's
//! readable registers in the register model's order; the analog image is
//! every analog-I2C register a reviewed `PhyI2cField` occupies, in the
//! order of its `register-image`. A request names a window of indices and
//! the reply carries their values. Hardware cross-checks compare them with
//! the vendor firmware's state after calibration.

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

/// The values of a window of the analog image.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhyAnalogImageBytes {
    pub first: u16,
    /// Registers of the whole analog image.
    pub length: u16,
    pub values: heapless::Vec<u8, PHY_REGISTER_IMAGE_WORDS>,
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

    #[test]
    fn a_full_analog_window_survives_the_wire_encoding() {
        let bytes = PhyAnalogImageBytes {
            first: 16,
            length: 40,
            values: (0..PHY_REGISTER_IMAGE_WORDS as u8).collect(),
        };
        let mut buffer = [0; 128];
        let encoded =
            postcard::to_slice(&Event::PhyAnalogImage(bytes.clone()), &mut buffer).unwrap();
        let decoded: Event = postcard::from_bytes(encoded).unwrap();
        assert_eq!(decoded, Event::PhyAnalogImage(bytes));
        let request = Command::PhyAnalogImage(PhyRegisterImageRequest {
            first: 16,
            count: 16,
        });
        let encoded = postcard::to_slice(&request, &mut buffer).unwrap();
        assert_eq!(postcard::from_bytes::<Command>(encoded).unwrap(), request);
    }
}
