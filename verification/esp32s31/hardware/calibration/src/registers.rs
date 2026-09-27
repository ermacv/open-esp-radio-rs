//! The calibrated register state the cross-check compares: the radio-PHY
//! partition image.
//!
//! The register model's own enumeration defines the image: every readable
//! register of the published `RadioPhyPeripherals` ownership partition, in
//! the order the production PAC reads it by index
//! (`RadioPhyRegisters::register_image`). The cross-check therefore names
//! each index exactly as production reads it, and holds no register list of
//! its own.
use crate::Result;
use std::path::Path;

/// Published SVD and PAC API policy, relative to the repository root.
const SVD: &str = "registers/esp32s31/published/radio.svd";
const API_POLICY: &str = "registers/esp32s31/policy/api.toml";
/// The ownership partition of the role-neutral RF and PHY registers.
pub const PARTITION: &str = "RadioPhyPeripherals";

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_partition_image_is_ordered_by_address() {
        let registers = partition(&crate::repository_root(), PARTITION).unwrap();
        assert!(!registers.is_empty());
        assert!(registers.windows(2).all(|w| w[0].address < w[1].address));
    }
}
