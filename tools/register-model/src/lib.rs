//! Versioned, multi-file register model independent of discovery evidence and XML.

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use svd_rs::{Device, MaybeArray, Peripheral, RegisterCluster, RegisterProperties, ValidateLevel};

mod model_validation;
mod pac_api;
mod pac_api_render;
mod pac_api_svd;
mod pac_bindings;
mod register_evidence;
mod register_lints;

pub use pac_api::{
    FixedRegisterImage, FixedRegisterWrite, FullRegisterWrite, InterruptSnapshot,
    MaskedRegisterModify, PacApiOptions, PacApiPack, RegisterImageWrite, ZeroBasedFieldWrite,
    ZeroRegisterWrite,
};
pub use pac_bindings::{generate_pac_binding_index, validate_pac_crate_name};
pub use register_evidence::{
    RegisterEvidenceCatalog, RegisterEvidenceRange, RegisterEvidenceSet, RegisterEvidenceSource,
};
pub use register_lints::RegisterLintPack;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Toml(#[from] toml_edit::TomlError),

    #[error(transparent)]
    TomlDeserialize(#[from] toml_edit::de::Error),

    #[error(transparent)]
    Svd(#[from] svd_rs::SvdError),
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::Message(value.to_owned())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SvdExportSummary {
    pub peripherals: usize,
    pub registers: usize,
    pub fields: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterModelManifest {
    pub schema: u32,
    #[serde(default = "default_address_space")]
    pub address_space: String,
    pub device: ModelDevice,
    pub fragments: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ModelDevice {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<svd_rs::Cpu>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_system_filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_definitions_prefix: Option<String>,
    pub address_unit_bits: u32,
    pub width: u32,
    #[serde(default)]
    pub register_defaults: RegisterProperties,
    #[serde(default = "default_svd_schema")]
    pub svd_schema: String,
    #[serde(default = "default_svd_schema_location")]
    pub svd_schema_location: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterModelFragment {
    pub schema: u32,
    pub peripherals: Vec<Peripheral>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub review: Vec<ReviewAnnotation>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReviewAnnotation {
    pub entity: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RegisterModel {
    device: Device,
    review: Vec<ReviewAnnotation>,
}

impl RegisterModel {
    pub fn is_model_file(path: &Path) -> Result<bool> {
        let input = fs::read_to_string(path)?;
        let document = input.parse::<toml_edit::DocumentMut>()?;
        Ok(document.get("schema").and_then(toml_edit::Item::as_integer) == Some(2))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let manifest: RegisterModelManifest = toml_edit::de::from_str(&input)?;
        if manifest.schema != 2 {
            return Err(format!("{} requires register model schema = 2", path.display()).into());
        }
        if manifest.fragments.is_empty() {
            return Err("register model requires at least one peripheral fragment".into());
        }
        if manifest.address_space.is_empty() {
            return Err("register model address-space must not be empty".into());
        }
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut seen_paths = BTreeSet::new();
        let mut fragments = Vec::with_capacity(manifest.fragments.len());
        let mut peripherals = Vec::new();
        for relative in &manifest.fragments {
            validate_relative_fragment(relative)?;
            if !seen_paths.insert(relative) {
                return Err(format!("duplicate register model fragment {relative:?}").into());
            }
            let fragment_path = base.join(relative);
            let input = fs::read_to_string(&fragment_path)?;
            let fragment: RegisterModelFragment = toml_edit::de::from_str(&input)?;
            if fragment.schema != 2 {
                return Err(
                    format!("{} requires fragment schema = 2", fragment_path.display()).into(),
                );
            }
            if fragment.peripherals.is_empty() {
                return Err(format!("{} contains no peripherals", fragment_path.display()).into());
            }
            peripherals.extend(fragment.peripherals.iter().cloned());
            fragments.push(fragment);
        }
        validate_peripheral_names(&peripherals)?;
        validate_review_annotations(&fragments)?;
        let review = fragments
            .into_iter()
            .flat_map(|fragment| fragment.review)
            .collect();
        let device = build_device(&manifest.device, peripherals)?;
        model_validation::validate_device(&device)?;
        let model = Self { device, review };
        model.register_identities()?;
        Ok(model)
    }

    pub fn render_svd(&self) -> Result<(String, SvdExportSummary)> {
        let mut output = svd_encoder::encode(&self.device)
            .map_err(|error| format!("failed to encode register model as SVD: {error}"))?;
        if !output.ends_with('\n') {
            output.push('\n');
        }
        let identities = self.register_identities()?;
        let fields = self
            .device
            .peripherals
            .iter()
            .map(count_peripheral_fields)
            .sum();
        Ok((
            output,
            SvdExportSummary {
                peripherals: self.device.peripherals.len(),
                registers: identities.len(),
                fields,
            },
        ))
    }

    pub fn register_identities(&self) -> Result<BTreeMap<(u64, u32), String>> {
        let mut output = BTreeMap::new();
        for peripheral in &self.device.peripherals {
            for peripheral in expand_peripheral(peripheral) {
                let inherited = merge_properties(
                    self.device.default_register_properties,
                    peripheral.default_register_properties,
                );
                if let Some(children) = &peripheral.registers {
                    collect_registers(
                        &mut output,
                        peripheral.base_address,
                        0,
                        inherited,
                        &peripheral.name,
                        children,
                    )?;
                }
            }
        }
        validate_expanded_register_layout(&output)?;
        Ok(output)
    }

    pub fn review(&self) -> &[ReviewAnnotation] {
        &self.review
    }

    pub fn validate_lints(&self, pack: &RegisterLintPack) -> Result<()> {
        pack.validate_device(&self.device)
    }
}

fn validate_review_annotations(fragments: &[RegisterModelFragment]) -> Result<()> {
    let mut known_entities = BTreeSet::new();
    for peripheral in fragments.iter().flat_map(|fragment| &fragment.peripherals) {
        collect_review_entities(peripheral, &mut known_entities);
    }
    let mut entities = BTreeSet::new();
    for annotation in fragments.iter().flat_map(|fragment| &fragment.review) {
        if annotation.entity.is_empty() || !entities.insert(annotation.entity.as_str()) {
            return Err(format!(
                "empty or duplicate register review entity {:?}",
                annotation.entity
            )
            .into());
        }
        if !known_entities.contains(annotation.entity.as_str()) {
            return Err(format!(
                "register review entity {:?} does not exist in the model",
                annotation.entity
            )
            .into());
        }
        if annotation
            .confidence
            .as_ref()
            .is_some_and(|confidence| confidence.is_empty())
        {
            return Err(format!(
                "register review entity {:?} has empty confidence",
                annotation.entity
            )
            .into());
        }
        let mut sources = BTreeSet::new();
        if annotation
            .sources
            .iter()
            .any(|source| source.is_empty() || !sources.insert(source))
        {
            return Err(format!(
                "register review entity {:?} has empty or duplicate sources",
                annotation.entity
            )
            .into());
        }
    }
    Ok(())
}

fn collect_review_entities(peripheral: &Peripheral, entities: &mut BTreeSet<String>) {
    entities.insert(peripheral.name.clone());
    for interrupt in &peripheral.interrupt {
        entities.insert(format!("{}.interrupt.{}", peripheral.name, interrupt.name));
    }
    if let Some(children) = &peripheral.registers {
        collect_child_review_entities(&peripheral.name, children, entities);
    }
}

fn collect_child_review_entities(
    parent: &str,
    children: &[RegisterCluster],
    entities: &mut BTreeSet<String>,
) {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let path = format!("{parent}.{}", register.name);
                entities.insert(path.clone());
                if let Some(fields) = &register.fields {
                    for field in fields {
                        let field_path = format!("{path}.{}", field.name);
                        entities.insert(field_path.clone());
                        for values in &field.enumerated_values {
                            for value in &values.values {
                                entities.insert(format!("{field_path}.{}", value.name));
                            }
                        }
                    }
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let path = format!("{parent}.{}", cluster.name);
                entities.insert(path.clone());
                collect_child_review_entities(&path, &cluster.children, entities);
            }
        }
    }
}

fn default_address_space() -> String {
    "cpu".to_owned()
}

fn default_svd_schema() -> String {
    "1.3".to_owned()
}

fn default_svd_schema_location() -> String {
    "CMSIS-SVD.xsd".to_owned()
}

fn validate_relative_fragment(value: &str) -> Result<()> {
    let path = PathBuf::from(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(
            format!("register model fragment must be a safe relative path: {value:?}").into(),
        );
    }
    Ok(())
}

fn validate_peripheral_names(peripherals: &[Peripheral]) -> Result<()> {
    let mut names = BTreeSet::new();
    for peripheral in peripherals {
        if !names.insert(peripheral.name.as_str()) {
            return Err(
                format!("duplicate register model peripheral {:?}", peripheral.name).into(),
            );
        }
    }
    Ok(())
}

fn build_device(metadata: &ModelDevice, peripherals: Vec<Peripheral>) -> Result<Device> {
    Ok(Device::builder()
        .name(metadata.name.clone())
        .vendor(metadata.vendor.clone())
        .vendor_id(metadata.vendor_id.clone())
        .series(metadata.series.clone())
        .version(metadata.version.clone())
        .description(metadata.description.clone())
        .license_text(metadata.license_text.clone())
        .cpu(metadata.cpu.clone())
        .header_system_filename(metadata.header_system_filename.clone())
        .header_definitions_prefix(metadata.header_definitions_prefix.clone())
        .address_unit_bits(metadata.address_unit_bits)
        .width(metadata.width)
        .default_register_properties(metadata.register_defaults)
        .peripherals(peripherals)
        .schema_version(metadata.svd_schema.clone())
        .no_namespace_schema_location(metadata.svd_schema_location.clone())
        .build(ValidateLevel::Strict)?)
}

fn expand_peripheral(peripheral: &Peripheral) -> Vec<svd_rs::PeripheralInfo> {
    match peripheral {
        MaybeArray::Single(info) => vec![info.clone()],
        MaybeArray::Array(info, dim) => svd_rs::peripheral::expand(info, dim).collect(),
    }
}

fn collect_registers(
    output: &mut BTreeMap<(u64, u32), String>,
    peripheral_base: u64,
    parent_offset: u64,
    inherited: RegisterProperties,
    path: &str,
    children: &[RegisterCluster],
) -> Result<()> {
    for child in children {
        match child {
            RegisterCluster::Register(register) => {
                let registers = match register {
                    MaybeArray::Single(info) => vec![info.clone()],
                    MaybeArray::Array(info, dim) => svd_rs::register::expand(info, dim).collect(),
                };
                for register in registers {
                    let properties = merge_properties(inherited, register.properties);
                    let width = properties.size.ok_or_else(|| {
                        format!("register {path}.{} has no inherited size", register.name)
                    })?;
                    let address = peripheral_base
                        .checked_add(parent_offset)
                        .and_then(|address| address.checked_add(u64::from(register.address_offset)))
                        .ok_or("register model address overflow")?;
                    let identity = format!("{path}.{}", register.name);
                    if let Some(previous) = output.insert((address, width), identity.clone()) {
                        return Err(format!(
                            "register model aliases {previous} and {identity} at {address:#010x}/{width}; explicit alias support is required"
                        )
                        .into());
                    }
                }
            }
            RegisterCluster::Cluster(cluster) => {
                let clusters = match cluster {
                    MaybeArray::Single(info) => vec![info.clone()],
                    MaybeArray::Array(info, dim) => svd_rs::cluster::expand(info, dim).collect(),
                };
                for cluster in clusters {
                    let offset = parent_offset
                        .checked_add(u64::from(cluster.address_offset))
                        .ok_or("register cluster address overflow")?;
                    let properties =
                        merge_properties(inherited, cluster.default_register_properties);
                    collect_registers(
                        output,
                        peripheral_base,
                        offset,
                        properties,
                        &format!("{path}.{}", cluster.name),
                        &cluster.children,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn validate_expanded_register_layout(registers: &BTreeMap<(u64, u32), String>) -> Result<()> {
    let mut previous: Option<(u64, u64, &str)> = None;
    for ((address, width), identity) in registers {
        if !(1..=128).contains(width) {
            return Err(format!(
                "register {identity} at {address:#010x} has unsupported width {width}"
            )
            .into());
        }
        let size_bytes = u64::from(*width).div_ceil(8);
        if address % size_bytes != 0 {
            return Err(format!(
                "register {identity} at {address:#010x} is not aligned to {size_bytes} bytes"
            )
            .into());
        }
        let end = address
            .checked_add(size_bytes)
            .ok_or("register model address overflow")?;
        if let Some((previous_start, previous_end, previous_identity)) = previous
            && *address < previous_end
        {
            return Err(format!(
                "physical register ranges overlap: {previous_identity} at \
                 {previous_start:#010x}..{previous_end:#010x} and {identity} at \
                 {address:#010x}..{end:#010x}; explicit alias support is required"
            )
            .into());
        }
        previous = Some((*address, end, identity));
    }
    Ok(())
}

fn merge_properties(parent: RegisterProperties, child: RegisterProperties) -> RegisterProperties {
    let mut merged = parent;
    if child.size.is_some() {
        merged.size = child.size;
    }
    if child.access.is_some() {
        merged.access = child.access;
    }
    if child.protection.is_some() {
        merged.protection = child.protection;
    }
    if child.reset_value.is_some() {
        merged.reset_value = child.reset_value;
    }
    if child.reset_mask.is_some() {
        merged.reset_mask = child.reset_mask;
    }
    merged
}

fn count_peripheral_fields(peripheral: &Peripheral) -> usize {
    let multiplier = match peripheral {
        MaybeArray::Single(_) => 1,
        MaybeArray::Array(_, dim) => dim.dim as usize,
    };
    peripheral
        .registers
        .as_deref()
        .map(count_fields)
        .unwrap_or(0)
        * multiplier
}

fn count_fields(children: &[RegisterCluster]) -> usize {
    children
        .iter()
        .map(|child| match child {
            RegisterCluster::Register(register) => {
                let multiplier = match register {
                    MaybeArray::Single(_) => 1,
                    MaybeArray::Array(_, dim) => dim.dim as usize,
                };
                register
                    .fields
                    .as_ref()
                    .map(|fields| {
                        fields
                            .iter()
                            .map(|field| match field {
                                MaybeArray::Single(_) => 1,
                                MaybeArray::Array(_, dim) => dim.dim as usize,
                            })
                            .sum::<usize>()
                            * multiplier
                    })
                    .unwrap_or(0)
            }
            RegisterCluster::Cluster(cluster) => {
                let multiplier = match cluster {
                    MaybeArray::Single(_) => 1,
                    MaybeArray::Array(_, dim) => dim.dim as usize,
                };
                count_fields(&cluster.children) * multiplier
            }
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanded_layout_rejects_unaligned_and_overlapping_registers() {
        let unaligned = BTreeMap::from([((0x1001, 32), "RADIO.UNALIGNED".to_owned())]);
        assert!(
            validate_expanded_register_layout(&unaligned)
                .unwrap_err()
                .to_string()
                .contains("not aligned")
        );

        let overlapping = BTreeMap::from([
            ((0x1000, 64), "RADIO.WIDE".to_owned()),
            ((0x1004, 32), "RADIO.NARROW".to_owned()),
        ]);
        assert!(
            validate_expanded_register_layout(&overlapping)
                .unwrap_err()
                .to_string()
                .contains("physical register ranges overlap")
        );
    }
}
