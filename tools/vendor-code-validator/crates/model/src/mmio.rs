//! SVD-derived register names plus independently supplied MMIO windows.

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Window {
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MmioRegisterMap {
    pub registers: Vec<Register>,
    pub windows: Vec<Window>,
}

impl MmioRegisterMap {
    pub fn load(path: &Path) -> Result<Self> {
        let xml = fs::read_to_string(path)?;
        let document = roxmltree::Document::parse(&xml)?;
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

        let mut windows = Vec::new();
        for node in document
            .descendants()
            .filter(|node| node.has_tag_name("window"))
        {
            let (Some(start), Some(end)) = (
                node.attribute("start").and_then(u32_literal),
                node.attribute("endExclusive").and_then(u32_literal),
            ) else {
                continue;
            };
            windows.push(Window { start, end });
        }
        Ok(Self { registers, windows })
    }

    pub fn load_all(paths: &[PathBuf]) -> Result<Self> {
        let mut combined = Self {
            registers: Vec::new(),
            windows: Vec::new(),
        };
        for path in paths {
            let map = Self::load(path)?;
            combined.registers.extend(map.registers);
            combined.windows.extend(map.windows);
        }
        combined
            .registers
            .sort_by_key(|register| (register.address, register.name.clone()));
        combined.registers.dedup();
        reject_register_collisions(&combined.registers)?;
        combined
            .windows
            .sort_by_key(|window| (window.start, window.end));
        combined.windows.dedup();
        Ok(combined)
    }

    pub fn contains_mmio(&self, address: u32) -> bool {
        self.windows
            .iter()
            .any(|window| address >= window.start && address < window.end)
    }

    pub fn register_name(&self, address: u32) -> String {
        let names: Vec<_> = self
            .registers
            .iter()
            .filter(|register| register.address == address)
            .map(|register| register.name.as_str())
            .collect();
        if names.is_empty() {
            "UNMAPPED".to_owned()
        } else {
            names.join("|")
        }
    }

    pub fn register(&self, address: u32) -> Option<&Register> {
        self.registers
            .binary_search_by_key(&address, |register| register.address)
            .ok()
            .map(|index| &self.registers[index])
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
            "open-radio-validator-standard-svd-{}.svd",
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
        let map = MmioRegisterMap::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(map.registers[0].address, 0x2010_0010);
        assert_eq!(map.registers[0].name, "RADIO.CONTROL");
        assert!(map.windows.is_empty());
    }
}
