//! The calibrated register state the cross-check compares: every readable
//! register of the published radio-PHY ownership partition.
//!
//! The partition's peripherals come from the reviewed PAC API policy and
//! their registers from the published SVD, so the selection follows the
//! register model production is written against and names no register of
//! its own.
use crate::Result;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use svd_parser::svd::{Access, MaybeArray};

/// Published SVD and PAC API policy, relative to the repository root.
const SVD: &str = "registers/esp32s31/published/radio.svd";
const API_POLICY: &str = "registers/esp32s31/policy/api.toml";
/// The ownership partition of the role-neutral RF and PHY registers.
pub const PARTITION: &str = "RadioPhyPeripherals";

/// One readable register, named by its first peripheral when several
/// published views share its address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Register {
    pub name: String,
    pub address: u32,
}

#[derive(Deserialize)]
struct Policy {
    #[serde(rename = "ownership-partitions")]
    partitions: Vec<Partition>,
}

#[derive(Deserialize)]
struct Partition {
    name: String,
    peripherals: Vec<String>,
}

/// The readable registers of `partition`, ascending by address.
pub fn partition(root: &Path, partition: &str) -> Result<Vec<Register>> {
    let policy: Policy = toml::from_str(&std::fs::read_to_string(root.join(API_POLICY))?)?;
    let members = policy
        .partitions
        .into_iter()
        .find(|p| p.name == partition)
        .ok_or_else(|| format!("the API policy has no partition {partition}"))?
        .peripherals;
    readable(&std::fs::read_to_string(root.join(SVD))?, &members)
}

/// Readable registers of `peripherals` in `svd`, deduplicated by address.
fn readable(svd: &str, peripherals: &[String]) -> Result<Vec<Register>> {
    let config = svd_parser::Config::default().expand(true);
    let device = svd_parser::parse_with_config(svd, &config)?;
    let mut registers = BTreeMap::new();
    for name in peripherals {
        let peripheral = device
            .peripherals
            .iter()
            .find(|p| &p.name == name)
            .ok_or_else(|| format!("the published SVD has no peripheral {name}"))?;
        let default = peripheral.default_register_properties.access;
        for register in peripheral.all_registers() {
            let MaybeArray::Single(info) = register else {
                return Err(format!("{name}.{} is an unexpanded array", register.name).into());
            };
            let access = info
                .properties
                .access
                .or(default)
                .unwrap_or(Access::ReadWrite);
            if !access.can_read() {
                continue;
            }
            let address = u32::try_from(peripheral.base_address)? + info.address_offset;
            registers
                .entry(address)
                .or_insert_with(|| format!("{name}.{}", info.name));
        }
    }
    Ok(registers
        .into_iter()
        .map(|(address, name)| Register { name, address })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SVD_TEXT: &str = r#"<?xml version="1.0"?>
<device><name>T</name><addressUnitBits>8</addressUnitBits><width>32</width>
<size>32</size><access>read-write</access><resetValue>0</resetValue><resetMask>0xffffffff</resetMask>
<peripherals>
<peripheral><name>A</name><baseAddress>0x1000</baseAddress><registers>
<register><name>RW</name><addressOffset>0x0</addressOffset></register>
<register><name>WO</name><addressOffset>0x4</addressOffset><access>write-only</access></register>
<register><name>RO</name><addressOffset>0x8</addressOffset><access>read-only</access></register>
</registers></peripheral>
<peripheral><name>B</name><baseAddress>0x1000</baseAddress><registers>
<register><name>ALIAS</name><addressOffset>0x0</addressOffset></register>
</registers></peripheral>
<peripheral><name>C</name><baseAddress>0x2000</baseAddress><registers>
<register><name>OTHER</name><addressOffset>0x0</addressOffset></register>
</registers></peripheral>
</peripherals></device>"#;

    #[test]
    fn readable_registers_of_the_members_once_per_address() {
        let registers = readable(SVD_TEXT, &["A".into(), "B".into()]).unwrap();
        assert_eq!(
            registers,
            [
                Register {
                    name: "A.RW".into(),
                    address: 0x1000
                },
                Register {
                    name: "A.RO".into(),
                    address: 0x1008
                },
            ]
        );
    }

    #[test]
    fn the_published_partition_is_selected() {
        let root = crate::repository_root();
        let registers = partition(&root, PARTITION).unwrap();
        assert!(!registers.is_empty());
        assert!(registers.windows(2).all(|w| w[0].address < w[1].address));
    }
}
