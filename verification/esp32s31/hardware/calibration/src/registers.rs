//! The calibrated register state the cross-check compares: the radio-PHY
//! partition image and the analog image.
//!
//! The register model's own enumeration defines the image: every readable
//! register of the published `RadioPhyPeripherals` ownership partition, in
//! the order the production PAC reads it by index
//! (`RadioPhyRegisters::register_image`). The cross-check therefore names
//! each index exactly as production reads it, and holds no register list of
//! its own. The analog image is likewise the model's `register-image` of
//! the reviewed `PhyI2cField` domain: every analog-I2C register a reviewed
//! field occupies, in the order the production PAC reads it
//! (`PhyI2cAddress::image`).
use crate::Result;
use std::path::Path;

/// Published SVD and PAC API policy, relative to the repository root.
const SVD: &str = "registers/esp32s31/published/radio.svd";
const API_POLICY: &str = "registers/esp32s31/policy/api.toml";
/// The ownership partition of the role-neutral RF and PHY registers.
pub const PARTITION: &str = "RadioPhyPeripherals";
/// The indirect-register domain of the reviewed analog-I2C fields.
pub const ANALOG_DOMAIN: &str = "PhyI2cField";

/// The register space a [`Register`] address lies in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// A memory-mapped radio-PHY register word.
    Mmio,
    /// An analog-I2C byte register, addressed by [`analog_address`].
    Analog,
}

impl Space {
    /// The vendor firmware's request for the register at `address`.
    pub fn request(self, address: u32) -> String {
        match self {
            Self::Mmio => crate::vendor::register_request(address),
            Self::Analog => {
                let (block, register) = analog_parts(address);
                crate::vendor::analog_request(block, register)
            }
        }
    }

    /// The reply line reporting `value` at `address`.
    pub fn line(self, address: u32, value: u32) -> String {
        match self {
            Self::Mmio => crate::vendor::register_line(address, value),
            Self::Analog => {
                let (block, register) = analog_parts(address);
                crate::vendor::analog_line(block, register, value as u8)
            }
        }
    }

    /// Every complete reply of `console` in this space, by address.
    pub fn replies(self, console: &str) -> Result<std::collections::BTreeMap<u32, u32>> {
        Ok(match self {
            Self::Mmio => crate::vendor::registers(console)?,
            Self::Analog => crate::vendor::analog(console)?
                .into_iter()
                .map(|((block, register), value)| {
                    (analog_address(block, register), u32::from(value))
                })
                .collect(),
        })
    }
}

/// The [`Register`] address of analog `register` of `block`.
pub fn analog_address(block: u8, register: u8) -> u32 {
    u32::from_be_bytes([0, 0, block, register])
}

/// The block and register of an analog [`Register`] address.
pub fn analog_parts(address: u32) -> (u8, u8) {
    let [_, _, block, register] = address.to_be_bytes();
    (block, register)
}

/// One register of the image, named by its published peripheral and
/// register (and array element).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Register {
    pub name: String,
    pub address: u32,
}

/// The image of `partition`, in production's index order.
pub fn partition(root: &Path, partition: &str) -> Result<Vec<Register>> {
    let policy = oer_register_model::PacApiPack::load(&root.join(API_POLICY))?;
    let svd = std::fs::read_to_string(root.join(SVD))?;
    Ok(policy
        .partition_readable_registers(&svd, partition)?
        .into_iter()
        .map(|register| Register {
            name: match register.element {
                Some(element) => format!(
                    "{}.{}",
                    register.peripheral,
                    register.register.replace("%s", &element.to_string())
                ),
                None => format!("{}.{}", register.peripheral, register.register),
            },
            address: register.address,
        })
        .collect())
}

/// The analog image of the indirect-register `domain`, in production's
/// index order.
pub fn analog(root: &Path, domain: &str) -> Result<Vec<Register>> {
    let policy = oer_register_model::PacApiPack::load(&root.join(API_POLICY))?;
    let domain = policy
        .indirect_register_field_domains
        .iter()
        .find(|candidate| candidate.name == domain && candidate.register_image)
        .ok_or_else(|| format!("no indirect-register domain {domain} with a register image"))?;
    Ok(domain
        .image_registers()
        .into_iter()
        .map(|(block, register)| Register {
            name: format!("{}.{block:02x}.{register:02x}", domain.name),
            address: analog_address(block, register),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_partition_image_is_ordered_by_address() {
        let registers = partition(&oer_process::built_root(), PARTITION).unwrap();
        assert!(!registers.is_empty());
        assert!(registers.windows(2).all(|w| w[0].address < w[1].address));
    }

    #[test]
    fn analog_replies_round_trip_through_their_space() {
        let image = analog(&oer_process::built_root(), ANALOG_DOMAIN).unwrap();
        assert!(image.windows(2).all(|w| w[0].address < w[1].address));
        let register = &image[0];
        let line = Space::Analog.line(register.address, 0xa5);
        assert_eq!(
            Space::Analog.replies(&line).unwrap()[&register.address],
            0xa5
        );
        assert!(Space::Analog.request(register.address).starts_with("a "));
    }
}
