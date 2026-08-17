//! Physical MMIO classification with optional register-catalog enrichment.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{Result, u32_literal};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Register {
    pub address: u32,
    pub name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegisterCatalog {
    pub registers: Vec<Register>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MmioRegion {
    pub name: String,
    pub start: u32,
    pub end: u32,
    pub readable: bool,
    pub writable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MmioMap {
    pub registers: Vec<Register>,
    pub regions: Vec<MmioRegion>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MmioAccessKind {
    Read,
    Write,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MmioAccessIdentity {
    pub region: String,
    pub address: u32,
    pub width: u8,
    pub register: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum MmioAccessError {
    #[error("unsupported MMIO access width {width}; expected 8, 16 or 32 bits")]
    UnsupportedWidth { width: u8 },
    #[error("MMIO {access} at {address:#010x} is outside every declared MMIO region")]
    Unclassified { access: &'static str, address: u32 },
    #[error(
        "MMIO {access} at {address:#010x} with width {width} crosses the boundary of region {region}"
    )]
    CrossesRegion {
        access: &'static str,
        address: u32,
        width: u8,
        region: String,
    },
    #[error("MMIO {access} at {address:#010x} is not permitted by region {region}")]
    PermissionDenied {
        access: &'static str,
        address: u32,
        region: String,
    },
}

impl RegisterCatalog {
    pub fn load(path: &Path) -> Result<Self> {
        let xml = fs::read_to_string(path)?;
        Self::parse(&xml)
    }

    pub fn parse(xml: &str) -> Result<Self> {
        let document = roxmltree::Document::parse(xml)?;
        let mut registers = Vec::new();
        for peripheral in document
            .descendants()
            .filter(|node| node.has_tag_name("peripheral"))
        {
            let Some(name) = child_text(peripheral, "name") else {
                continue;
            };
            let Some(base) = child_text(peripheral, "baseAddress").and_then(u32_literal) else {
                continue;
            };
            let Some(container) = peripheral
                .children()
                .find(|node| node.has_tag_name("registers"))
            else {
                continue;
            };
            collect_registers(container, base, name, &mut registers)?;
        }
        registers.sort_by_key(|register| (register.address, register.name.clone()));

        Ok(Self { registers })
    }

    pub fn load_all(paths: &[PathBuf]) -> Result<Self> {
        let mut combined = Self {
            registers: Vec::new(),
        };
        for path in paths {
            combined.merge(Self::load(path)?)?;
        }
        Ok(combined)
    }

    pub fn merge(&mut self, other: Self) -> Result<()> {
        self.registers.extend(other.registers);
        self.registers
            .sort_by_key(|register| (register.address, register.name.clone()));
        self.registers.dedup();
        reject_register_collisions(&self.registers)?;
        Ok(())
    }

    pub fn register(&self, address: u32) -> Option<&Register> {
        self.registers
            .binary_search_by_key(&address, |register| register.address)
            .ok()
            .map(|index| &self.registers[index])
    }
}

impl MmioMap {
    pub fn new(catalog: RegisterCatalog, mut regions: Vec<MmioRegion>) -> Result<Self> {
        regions.sort_by_key(|region| (region.start, region.end, region.name.clone()));
        for pair in regions.windows(2) {
            let [left, right] = pair else {
                unreachable!("window pair has two elements")
            };
            if right.start < left.end {
                return Err(
                    format!("MMIO regions {} and {} overlap", left.name, right.name).into(),
                );
            }
        }
        Ok(Self {
            registers: catalog.registers,
            regions,
        })
    }

    pub fn load(path: &Path) -> Result<Self> {
        Ok(Self {
            registers: RegisterCatalog::load(path)?.registers,
            regions: Vec::new(),
        })
    }

    pub fn parse(xml: &str) -> Result<Self> {
        Ok(Self {
            registers: RegisterCatalog::parse(xml)?.registers,
            regions: Vec::new(),
        })
    }

    pub fn load_all(paths: &[PathBuf]) -> Result<Self> {
        Ok(Self {
            registers: RegisterCatalog::load_all(paths)?.registers,
            regions: Vec::new(),
        })
    }

    pub fn merge(&mut self, other: Self) -> Result<()> {
        let mut catalog = RegisterCatalog {
            registers: std::mem::take(&mut self.registers),
        };
        catalog.merge(RegisterCatalog {
            registers: other.registers,
        })?;
        self.registers = catalog.registers;
        self.regions.extend(other.regions);
        self.regions
            .sort_by_key(|region| (region.start, region.end, region.name.clone()));
        self.regions.dedup();
        Ok(())
    }

    pub fn contains_mmio(&self, address: u32) -> bool {
        self.regions
            .iter()
            .any(|region| address >= region.start && address < region.end)
    }

    pub fn intersects_mmio(&self, address: u32, width: u8) -> bool {
        let bytes = match width {
            8 => 1_u64,
            16 => 2,
            32 => 4,
            _ => return false,
        };
        let start = u64::from(address);
        let end = start + bytes;
        self.regions
            .iter()
            .any(|region| start < u64::from(region.end) && u64::from(region.start) < end)
    }

    pub fn register_name(&self, address: u32) -> Option<&str> {
        self.register(address)
            .map(|register| register.name.as_str())
    }

    /// Human-facing register label for structural reports that retain the
    /// historical `UNMAPPED` spelling as presentation, never as coverage
    /// state.
    pub fn display_register_name(&self, address: u32) -> String {
        self.register_name(address).unwrap_or("UNMAPPED").to_owned()
    }

    pub fn register(&self, address: u32) -> Option<&Register> {
        self.registers
            .binary_search_by_key(&address, |register| register.address)
            .ok()
            .map(|index| &self.registers[index])
    }

    pub fn classify_access(
        &self,
        address: u32,
        width: u8,
        kind: MmioAccessKind,
    ) -> std::result::Result<MmioAccessIdentity, MmioAccessError> {
        let bytes = match width {
            8 => 1,
            16 => 2,
            32 => 4,
            _ => return Err(MmioAccessError::UnsupportedWidth { width }),
        };
        let access = match kind {
            MmioAccessKind::Read => "read",
            MmioAccessKind::Write => "write",
        };
        let Some(region) = self
            .regions
            .iter()
            .find(|region| address >= region.start && address < region.end)
        else {
            if let Some(region) = self.regions.iter().find(|region| {
                u64::from(address) < u64::from(region.end)
                    && u64::from(region.start) < u64::from(address) + u64::from(bytes)
            }) {
                return Err(MmioAccessError::CrossesRegion {
                    access,
                    address,
                    width,
                    region: region.name.clone(),
                });
            }
            return Err(MmioAccessError::Unclassified { access, address });
        };
        let Some(end) = address.checked_add(bytes) else {
            return Err(MmioAccessError::CrossesRegion {
                access,
                address,
                width,
                region: region.name.clone(),
            });
        };
        if end > region.end {
            return Err(MmioAccessError::CrossesRegion {
                access,
                address,
                width,
                region: region.name.clone(),
            });
        }
        let permitted = match kind {
            MmioAccessKind::Read => region.readable,
            MmioAccessKind::Write => region.writable,
        };
        if !permitted {
            return Err(MmioAccessError::PermissionDenied {
                access,
                address,
                region: region.name.clone(),
            });
        }
        Ok(MmioAccessIdentity {
            region: region.name.clone(),
            address,
            width,
            register: self.register_name(address).map(str::to_owned),
        })
    }
}

pub fn reject_register_collisions(registers: &[Register]) -> Result<()> {
    for registers in registers.chunk_by(|left, right| left.address == right.address) {
        if registers.len() > 1 {
            let names = registers
                .iter()
                .map(|register| register.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "conflicting SVD register definitions at {:#010x}: {names}",
                registers[0].address
            )
            .into());
        }
    }
    Ok(())
}

fn child_text<'a, 'input>(node: roxmltree::Node<'a, 'input>, tag: &str) -> Option<&'a str> {
    node.children()
        .find(|child| child.has_tag_name(tag))
        .and_then(|child| child.text())
        .map(str::trim)
}

fn collect_registers(
    container: roxmltree::Node<'_, '_>,
    base: u32,
    prefix: &str,
    output: &mut Vec<Register>,
) -> Result<()> {
    for node in container.children().filter(roxmltree::Node::is_element) {
        if node.has_tag_name("register") {
            let name = child_text(node, "name").ok_or("SVD register has no name")?;
            let offset = child_text(node, "addressOffset")
                .and_then(u32_literal)
                .ok_or("SVD register has no addressOffset")?;
            let dim = child_text(node, "dim").and_then(u32_literal).unwrap_or(1);
            let increment = child_text(node, "dimIncrement")
                .and_then(u32_literal)
                .unwrap_or(0);
            for index in 0..dim {
                output.push(Register {
                    address: base.wrapping_add(offset).wrapping_add(index * increment),
                    name: if dim == 1 {
                        format!("{prefix}.{name}")
                    } else {
                        format!("{prefix}.{}", name.replace("%s", &index.to_string()))
                    },
                });
            }
        } else if node.has_tag_name("cluster") {
            let name = child_text(node, "name").ok_or("SVD cluster has no name")?;
            let offset = child_text(node, "addressOffset")
                .and_then(u32_literal)
                .ok_or("SVD cluster has no addressOffset")?;
            collect_registers(
                node,
                base.wrapping_add(offset),
                &format!("{prefix}.{name}"),
                output,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_svd_does_not_need_a_vendor_address_window_extension() {
        let path = std::env::temp_dir().join(format!(
            "open-radio-blobray-standard-svd-{}.svd",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"<?xml version="1.0"?>
<device>
  <peripherals>
    <peripheral>
      <name>RADIO</name>
      <baseAddress>0x20100000</baseAddress>
      <registers>
        <register>
          <name>CONTROL</name>
          <addressOffset>0x10</addressOffset>
        </register>
      </registers>
    </peripheral>
  </peripherals>
</device>"#,
        )
        .unwrap();
        let map = MmioMap::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(map.registers[0].address, 0x2010_0010);
        assert_eq!(map.registers[0].name, "RADIO.CONTROL");
        assert!(map.regions.is_empty());
    }

    fn physical_map(readable: bool, writable: bool) -> MmioMap {
        MmioMap {
            registers: Vec::new(),
            regions: vec![MmioRegion {
                name: "radio".to_owned(),
                start: 0x4000,
                end: 0x4004,
                readable,
                writable,
            }],
        }
    }

    #[test]
    fn classified_access_does_not_require_a_register_name() {
        let identity = physical_map(true, true)
            .classify_access(0x4000, 32, MmioAccessKind::Read)
            .unwrap();
        assert_eq!(identity.region, "radio");
        assert_eq!(identity.address, 0x4000);
        assert_eq!(identity.width, 32);
        assert_eq!(identity.register, None);
    }

    #[test]
    fn classification_checks_the_full_access_range_and_permissions() {
        assert!(matches!(
            physical_map(true, true).classify_access(0x4002, 32, MmioAccessKind::Read),
            Err(MmioAccessError::CrossesRegion { .. })
        ));
        assert!(matches!(
            physical_map(true, true).classify_access(0x3ffe, 32, MmioAccessKind::Read),
            Err(MmioAccessError::CrossesRegion { .. })
        ));
        assert!(matches!(
            physical_map(true, false).classify_access(0x4000, 32, MmioAccessKind::Write),
            Err(MmioAccessError::PermissionDenied { .. })
        ));
        assert!(matches!(
            physical_map(false, true).classify_access(0x4000, 32, MmioAccessKind::Read),
            Err(MmioAccessError::PermissionDenied { .. })
        ));
    }
}
