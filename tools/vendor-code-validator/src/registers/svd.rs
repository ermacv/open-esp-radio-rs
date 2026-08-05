//! Deterministic CMSIS-SVD materialization from facts plus reviewed metadata.

use std::{collections::BTreeMap, fmt::Write as _, fs, path::Path};

use super::{
    FactRange, FieldOverlay, RegisterFact, RegisterFacts, RegisterOverlay, RegisterOverlayFile,
    RegisterStatus, identifier_from, validate_identifier,
};
use crate::Result;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterWorkspace {
    pub(crate) facts: RegisterFacts,
    pub(crate) overlay: RegisterOverlayFile,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegisterWorkspaceSummary {
    pub(crate) ranges: usize,
    pub(crate) observed: usize,
    pub(crate) reviewed: usize,
    pub(crate) ignored: usize,
    pub(crate) manual: usize,
    pub(crate) unreviewed: usize,
    pub(crate) fields: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SvdExportSummary {
    pub(crate) peripherals: usize,
    pub(crate) registers: usize,
    pub(crate) fields: usize,
}

impl RegisterWorkspace {
    pub(crate) fn load(facts_path: &Path, overlay_path: &Path) -> Result<Self> {
        let facts = RegisterFacts::load(facts_path)?;
        let overlay = RegisterOverlayFile::load(overlay_path, &facts)?;
        let workspace = Self { facts, overlay };
        workspace.materialize(false)?;
        Ok(workspace)
    }

    pub(crate) fn summary(&self) -> RegisterWorkspaceSummary {
        let reviewed = self
            .overlay
            .registers
            .iter()
            .filter(|register| register.status == RegisterStatus::Reviewed)
            .count();
        let ignored = self
            .overlay
            .registers
            .iter()
            .filter(|register| register.status == RegisterStatus::Ignored)
            .count();
        let manual = self
            .overlay
            .registers
            .iter()
            .filter(|register| register.status == RegisterStatus::Manual)
            .count();
        RegisterWorkspaceSummary {
            ranges: self.facts.ranges.len(),
            observed: self.facts.registers.len(),
            reviewed,
            ignored,
            manual,
            unreviewed: self
                .facts
                .registers
                .len()
                .saturating_sub(reviewed + ignored),
            fields: self
                .overlay
                .registers
                .iter()
                .map(|register| register.fields.len())
                .sum(),
        }
    }

    pub(crate) fn write_svd(&self, path: &Path, reviewed_only: bool) -> Result<SvdExportSummary> {
        let peripherals = self.materialize(reviewed_only)?;
        if peripherals.is_empty() {
            return Err("SVD export selected no registers".into());
        }
        let mut output = String::new();
        output.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        output.push_str(
            "<device schemaVersion=\"1.3\" xmlns:xs=\"http://www.w3.org/2001/XMLSchema-instance\" xs:noNamespaceSchemaLocation=\"CMSIS-SVD.xsd\">\n",
        );
        element(
            &mut output,
            1,
            "vendor",
            self.overlay.device.vendor.as_deref(),
        );
        element(
            &mut output,
            1,
            "name",
            Some(self.overlay.device.name.as_str()),
        );
        element(
            &mut output,
            1,
            "version",
            Some(self.overlay.device.version.as_str()),
        );
        element(
            &mut output,
            1,
            "description",
            Some(self.overlay.device.description.as_str()),
        );
        element(
            &mut output,
            1,
            "addressUnitBits",
            Some(&self.overlay.device.address_unit_bits.to_string()),
        );
        element(
            &mut output,
            1,
            "width",
            Some(&self.overlay.device.width.to_string()),
        );
        output.push_str("  <peripherals>\n");
        for peripheral in &peripherals {
            write_peripheral(&mut output, peripheral);
        }
        output.push_str("  </peripherals>\n</device>\n");
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, output)?;
        Ok(SvdExportSummary {
            peripherals: peripherals.len(),
            registers: peripherals
                .iter()
                .map(|peripheral| peripheral.registers.len())
                .sum(),
            fields: peripherals
                .iter()
                .flat_map(|peripheral| &peripheral.registers)
                .map(|register| register.fields.len())
                .sum(),
        })
    }

    fn materialize(&self, reviewed_only: bool) -> Result<Vec<OutputPeripheral<'_>>> {
        let peripheral_overlays = self
            .overlay
            .peripherals
            .iter()
            .map(|peripheral| (peripheral.range.as_str(), peripheral))
            .collect::<BTreeMap<_, _>>();
        let register_overlays = self
            .overlay
            .registers
            .iter()
            .map(|register| ((register.address, register.width), register))
            .collect::<BTreeMap<_, _>>();

        let mut peripheral_names = BTreeMap::<String, String>::new();
        for peripheral in &self.overlay.peripherals {
            peripheral_names.insert(peripheral.range.clone(), peripheral.name.clone());
        }
        for range in &self.facts.ranges {
            if peripheral_names.contains_key(&range.name) {
                continue;
            }
            let base = identifier_from(&range.name).to_ascii_uppercase();
            let mut name = base.clone();
            let mut suffix = 2usize;
            while peripheral_names.values().any(|used| used == &name) {
                name = format!("{base}_{suffix}");
                suffix += 1;
            }
            peripheral_names.insert(range.name.clone(), name);
        }

        let mut output = self
            .facts
            .ranges
            .iter()
            .map(|range| OutputPeripheral {
                range,
                name: peripheral_names
                    .get(&range.name)
                    .expect("every fact range has a materialized name")
                    .clone(),
                description: peripheral_overlays
                    .get(range.name.as_str())
                    .and_then(|overlay| overlay.description.clone())
                    .unwrap_or_else(|| {
                        format!(
                            "MMIO discovery range {:#010x}..{:#010x}",
                            range.start, range.end
                        )
                    }),
                registers: Vec::new(),
            })
            .collect::<Vec<_>>();

        for fact in &self.facts.registers {
            let overlay = register_overlays.get(&(fact.address, fact.width)).copied();
            if overlay.is_some_and(|overlay| overlay.status == RegisterStatus::Ignored)
                || reviewed_only
                    && !overlay.is_some_and(|overlay| overlay.status == RegisterStatus::Reviewed)
            {
                continue;
            }
            let range = self
                .facts
                .range_for(fact.address)
                .expect("validated fact belongs to one range");
            let peripheral = output
                .iter_mut()
                .find(|peripheral| peripheral.range.name == range.name)
                .expect("every fact range has an output peripheral");
            peripheral
                .registers
                .push(materialize_register(range, fact, overlay));
        }
        for overlay in self
            .overlay
            .registers
            .iter()
            .filter(|register| register.status == RegisterStatus::Manual)
        {
            let range = self
                .facts
                .range_for(overlay.address)
                .expect("validated manual register belongs to one range");
            let peripheral = output
                .iter_mut()
                .find(|peripheral| peripheral.range.name == range.name)
                .expect("every fact range has an output peripheral");
            peripheral
                .registers
                .push(materialize_manual_register(overlay));
        }
        for peripheral in &mut output {
            peripheral
                .registers
                .sort_by_key(|register| (register.address, register.width, register.name.clone()));
            let mut names = BTreeMap::new();
            for register in &peripheral.registers {
                if let Some(address) = names.insert(register.name.as_str(), register.address) {
                    return Err(format!(
                        "register name {:?} is duplicated at {address:#010x} and {:#010x} in peripheral {}",
                        register.name, register.address, peripheral.name
                    )
                    .into());
                }
            }
        }
        output.retain(|peripheral| !peripheral.registers.is_empty());
        Ok(output)
    }
}

struct OutputPeripheral<'a> {
    range: &'a FactRange,
    name: String,
    description: String,
    registers: Vec<OutputRegister<'a>>,
}

struct OutputRegister<'a> {
    address: u32,
    width: u8,
    name: String,
    description: String,
    access: Option<&'a str>,
    reset_value: Option<u32>,
    reset_mask: Option<u32>,
    fields: &'a [FieldOverlay],
}

fn materialize_register<'a>(
    range: &FactRange,
    fact: &RegisterFact,
    overlay: Option<&'a RegisterOverlay>,
) -> OutputRegister<'a> {
    let name = overlay
        .and_then(|overlay| overlay.name.clone())
        .unwrap_or_else(|| fact_name(range, fact));
    OutputRegister {
        address: fact.address,
        width: fact.width,
        name,
        description: overlay
            .and_then(|overlay| overlay.description.clone())
            .unwrap_or_else(|| {
                format!(
                    "Unreviewed MMIO observation; reads={}, writes={}",
                    fact.reads, fact.writes
                )
            }),
        access: overlay.and_then(|overlay| overlay.access.as_deref()),
        reset_value: overlay.and_then(|overlay| overlay.reset_value),
        reset_mask: overlay.and_then(|overlay| overlay.reset_mask),
        fields: overlay.map_or(&[], |overlay| overlay.fields.as_slice()),
    }
}

fn materialize_manual_register(overlay: &RegisterOverlay) -> OutputRegister<'_> {
    OutputRegister {
        address: overlay.address,
        width: overlay.width,
        name: overlay
            .name
            .clone()
            .expect("validated manual register has a name"),
        description: overlay
            .description
            .clone()
            .unwrap_or_else(|| "Manually added register".to_owned()),
        access: overlay.access.as_deref(),
        reset_value: overlay.reset_value,
        reset_mask: overlay.reset_mask,
        fields: &overlay.fields,
    }
}

fn fact_name(range: &FactRange, fact: &RegisterFact) -> String {
    let catalog = fact
        .catalog_name
        .rsplit('.')
        .next()
        .filter(|name| *name != "UNMAPPED")
        .filter(|name| validate_identifier(name, "catalog register name").is_ok());
    catalog
        .map(str::to_owned)
        .unwrap_or_else(|| format!("REG_{:08X}_W{}", fact.address - range.start, fact.width))
}

fn write_peripheral(output: &mut String, peripheral: &OutputPeripheral<'_>) {
    output.push_str("    <peripheral>\n");
    element(output, 3, "name", Some(&peripheral.name));
    element(output, 3, "description", Some(&peripheral.description));
    element(
        output,
        3,
        "baseAddress",
        Some(&format!("{:#010x}", peripheral.range.start)),
    );
    output.push_str("      <registers>\n");
    for register in &peripheral.registers {
        output.push_str("        <register>\n");
        element(output, 5, "name", Some(&register.name));
        element(output, 5, "description", Some(&register.description));
        element(
            output,
            5,
            "addressOffset",
            Some(&format!("{:#x}", register.address - peripheral.range.start)),
        );
        element(output, 5, "size", Some(&register.width.to_string()));
        element(output, 5, "access", register.access);
        element(
            output,
            5,
            "resetValue",
            register
                .reset_value
                .map(|value| format!("{value:#x}"))
                .as_deref(),
        );
        element(
            output,
            5,
            "resetMask",
            register
                .reset_mask
                .map(|value| format!("{value:#x}"))
                .as_deref(),
        );
        if !register.fields.is_empty() {
            output.push_str("          <fields>\n");
            for field in register.fields {
                write_field(output, field);
            }
            output.push_str("          </fields>\n");
        }
        output.push_str("        </register>\n");
    }
    output.push_str("      </registers>\n    </peripheral>\n");
}

fn write_field(output: &mut String, field: &FieldOverlay) {
    output.push_str("            <field>\n");
    element(output, 7, "name", Some(&field.name));
    element(output, 7, "description", field.description.as_deref());
    element(output, 7, "bitOffset", Some(&field.lsb.to_string()));
    element(output, 7, "bitWidth", Some(&field.width.to_string()));
    element(output, 7, "access", field.access.as_deref());
    element(
        output,
        7,
        "modifiedWriteValues",
        field.modified_write_values.as_deref(),
    );
    element(output, 7, "readAction", field.read_action.as_deref());
    output.push_str("            </field>\n");
}

fn element(output: &mut String, indent: usize, name: &str, value: Option<&str>) {
    let Some(value) = value else {
        return;
    };
    writeln!(
        output,
        "{}<{name}>{}</{name}>",
        "  ".repeat(indent),
        xml_escape(value)
    )
    .expect("writing to String cannot fail");
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
