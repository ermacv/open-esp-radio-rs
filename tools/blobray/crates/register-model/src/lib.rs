//! Versioned, multi-file register model independent of discovery evidence and XML.

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    ops::Range,
    path::{Component, Path, PathBuf},
};

use open_radio_vendor_review::{AssertionValue, EffectiveAssertion, ReviewKnowledge};
use serde::{Deserialize, Serialize};
use svd_rs::{
    Access, Device, FieldInfo, MaybeArray, ModifiedWriteValues, Peripheral, RegisterCluster,
    RegisterInfo, RegisterProperties, ValidateLevel,
};

mod model_validation;
mod pac_api;
mod pac_api_render;
mod pac_api_svd;
mod pac_bindings;
mod register_evidence;
mod register_lints;

pub use pac_api::{
    BoundedDomain, EnumDomain, EnumValue, FeatureModule, FixedRegisterImage, FixedRegisterWrite,
    FlagDomain, FlagValue, FullRegisterWrite, InterruptSnapshot, MaskedRegisterModify,
    OpaqueDomain, OwnershipPartition, PacApiOptions, PacApiPack, RegisterImageWrite,
    ZeroBasedFieldWrite, ZeroRegisterWrite,
};
pub use pac_bindings::{generate_pac_binding_index, validate_pac_crate_name};
pub use register_evidence::{
    RegisterEvidenceCatalog, RegisterEvidenceRange, RegisterEvidenceSet, RegisterEvidenceSource,
};
pub use register_lints::RegisterLintPack;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid register model: {0}")]
    Invalid(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Toml(#[from] toml_edit::TomlError),

    #[error(transparent)]
    TomlDeserialize(#[from] toml_edit::de::Error),

    #[error(transparent)]
    TomlSerialize(#[from] toml_edit::ser::Error),

    #[error(transparent)]
    Svd(#[from] svd_rs::SvdError),

    #[error("invalid {kind} {path}: {reason}")]
    Manifest {
        kind: &'static str,
        path: PathBuf,
        reason: String,
        span: Option<Range<usize>>,
    },
}

impl Error {
    fn message(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    fn manifest(kind: &'static str, path: &Path, error: impl std::fmt::Display) -> Self {
        Self::manifest_span(kind, path, error, None)
    }

    fn manifest_span(
        kind: &'static str,
        path: &Path,
        error: impl std::fmt::Display,
        span: Option<Range<usize>>,
    ) -> Self {
        Self::Manifest {
            kind,
            path: path.to_owned(),
            reason: error.to_string(),
            span,
        }
    }

    /// Returns source-neutral manifest diagnostic data for a presentation layer.
    pub fn manifest_diagnostic(&self) -> Option<(&'static str, &Path, &str, Option<Range<usize>>)> {
        match self {
            Self::Manifest {
                kind,
                path,
                reason,
                span,
            } => Some((*kind, path, reason, span.clone())),
            _ => None,
        }
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
    pub provenance: Option<open_radio_vendor_contracts::FactProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy: Option<open_radio_vendor_contracts::FactAccuracy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completeness: Option<open_radio_vendor_contracts::FactCompleteness>,
}

#[derive(Clone, Debug)]
pub struct RegisterModel {
    address_space: String,
    device: Device,
    review: Vec<ReviewAnnotation>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReviewApplicationSummary {
    pub matched: usize,
    pub changed: usize,
    pub ignored: usize,
}

impl RegisterModel {
    /// Returns every file whose contents define this model.
    ///
    /// Consumers that cache derived register names or layouts must depend on
    /// the fragments as well as the top-level manifest.  Depending on the
    /// manifest alone leaves derived MMIO/IR documents stale after a fragment
    /// changes without changing the fragment list.
    pub fn input_paths(path: &Path) -> Result<Vec<PathBuf>> {
        let input = fs::read_to_string(path)?;
        let document = input
            .parse::<toml_edit::Document<String>>()
            .map_err(|error| {
                let span = error.span();
                Error::manifest_span("register model manifest", path, error, span)
            })?;
        let manifest: RegisterModelManifest = toml_edit::de::from_str(&input).map_err(|error| {
            let span = error.span();
            Error::manifest_span("register model manifest", path, error, span)
        })?;
        if manifest.schema != 2 {
            return Err(Error::manifest_span(
                "register model manifest",
                path,
                "requires schema = 2",
                document.get("schema").and_then(toml_edit::Item::span),
            ));
        }
        if manifest.fragments.is_empty() {
            return Err(Error::manifest_span(
                "register model manifest",
                path,
                "requires at least one peripheral fragment",
                document.get("fragments").and_then(toml_edit::Item::span),
            ));
        }

        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut seen = BTreeSet::new();
        let mut paths = Vec::with_capacity(manifest.fragments.len() + 1);
        paths.push(path.to_owned());
        for relative in &manifest.fragments {
            validate_relative_fragment(relative)
                .map_err(|error| Error::manifest("register model manifest", path, error))?;
            if !seen.insert(relative) {
                return Err(Error::manifest(
                    "register model manifest",
                    path,
                    format!("duplicate fragment {relative:?}"),
                ));
            }
            paths.push(base.join(relative));
        }
        Ok(paths)
    }

    pub fn is_model_file(path: &Path) -> Result<bool> {
        let input = fs::read_to_string(path)?;
        let document = input.parse::<toml_edit::DocumentMut>().map_err(|error| {
            let span = error.span();
            Error::manifest_span("register model manifest", path, error, span)
        })?;
        Ok(document.get("schema").and_then(toml_edit::Item::as_integer) == Some(2))
    }

    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let document = input
            .parse::<toml_edit::Document<String>>()
            .map_err(|error| {
                let span = error.span();
                Error::manifest_span("register model manifest", path, error, span)
            })?;
        let manifest: RegisterModelManifest = toml_edit::de::from_str(&input).map_err(|error| {
            let span = error.span();
            Error::manifest_span("register model manifest", path, error, span)
        })?;
        if manifest.schema != 2 {
            return Err(Error::manifest_span(
                "register model manifest",
                path,
                "requires schema = 2",
                document.get("schema").and_then(toml_edit::Item::span),
            ));
        }
        if manifest.fragments.is_empty() {
            return Err(Error::manifest_span(
                "register model manifest",
                path,
                "requires at least one peripheral fragment",
                document.get("fragments").and_then(toml_edit::Item::span),
            ));
        }
        if manifest.address_space.is_empty() {
            return Err(Error::manifest_span(
                "register model manifest",
                path,
                "address-space must not be empty",
                document
                    .get("address-space")
                    .and_then(toml_edit::Item::span),
            ));
        }
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut seen_paths = BTreeSet::new();
        let mut fragments = Vec::with_capacity(manifest.fragments.len());
        let mut peripherals = Vec::new();
        for relative in &manifest.fragments {
            validate_relative_fragment(relative)
                .map_err(|error| Error::manifest("register model manifest", path, error))?;
            if !seen_paths.insert(relative) {
                return Err(Error::manifest(
                    "register model manifest",
                    path,
                    format!("duplicate fragment {relative:?}"),
                ));
            }
            let fragment_path = base.join(relative);
            let input = fs::read_to_string(&fragment_path)?;
            let document = input
                .parse::<toml_edit::Document<String>>()
                .map_err(|error| {
                    let span = error.span();
                    Error::manifest_span("register model fragment", &fragment_path, error, span)
                })?;
            let fragment: RegisterModelFragment =
                toml_edit::de::from_str(&input).map_err(|error| {
                    let span = error.span();
                    Error::manifest_span("register model fragment", &fragment_path, error, span)
                })?;
            if fragment.schema != 2 {
                return Err(Error::manifest_span(
                    "register model fragment",
                    &fragment_path,
                    "requires schema = 2",
                    document.get("schema").and_then(toml_edit::Item::span),
                ));
            }
            if fragment.peripherals.is_empty() {
                return Err(Error::manifest_span(
                    "register model fragment",
                    &fragment_path,
                    "contains no peripherals",
                    document.get("peripherals").and_then(toml_edit::Item::span),
                ));
            }
            peripherals.extend(fragment.peripherals.iter().cloned());
            fragments.push(fragment);
        }
        validate_peripheral_names(&peripherals)
            .map_err(|error| Error::manifest("register model", path, error))?;
        validate_review_annotations(&fragments)
            .map_err(|error| Error::manifest("register model", path, error))?;
        let review = fragments
            .into_iter()
            .flat_map(|fragment| fragment.review)
            .collect();
        let device = build_device(&manifest.device, peripherals)
            .map_err(|error| Error::manifest("register model", path, error))?;
        model_validation::validate_device(&device)
            .map_err(|error| Error::manifest("register model", path, error))?;
        let model = Self {
            address_space: manifest.address_space,
            device,
            review,
        };
        model
            .register_identities()
            .map_err(|error| Error::manifest("register model", path, error))?;
        Ok(model)
    }

    pub fn render_svd(&self) -> Result<(String, SvdExportSummary)> {
        let mut output = svd_encoder::encode(&self.device).map_err(|error| {
            Error::message(format!("failed to encode register model as SVD: {error}"))
        })?;
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

    /// Apply sparse reviewed register facts in deterministic dependency order.
    ///
    /// Assertions for other domains/address spaces are ignored. A known
    /// register assertion must resolve to exactly one physical register; array
    /// instances fail closed until an array-wide subject vocabulary is
    /// introduced instead of silently editing every instance.
    pub fn apply_review_knowledge(
        &mut self,
        knowledge: &ReviewKnowledge,
    ) -> Result<ReviewApplicationSummary> {
        let mut assertions = knowledge.assertions().values().collect::<Vec<_>>();
        assertions.sort_by_key(|assertion| (assertion_priority(&assertion.kind), &assertion.id));
        let mut summary = ReviewApplicationSummary::default();
        for assertion in assertions {
            if !is_register_assertion(&assertion.kind) {
                summary.ignored += 1;
                continue;
            }
            let subject = RegisterSubject::parse(&assertion.subject).map_err(|reason| {
                Error::message(format!(
                    "reviewed assertion {:?} has invalid register subject: {reason}",
                    assertion.id
                ))
            })?;
            if subject.address_space != self.address_space {
                summary.ignored += 1;
                continue;
            }
            let old_identities = self.register_identities()?;
            let old_identity = old_identities
                .get(&(subject.address, subject.width))
                .cloned()
                .ok_or_else(|| {
                    Error::message(format!(
                        "reviewed assertion {:?} targets absent register {:#010x}/{} in address space {:?}",
                        assertion.id, subject.address, subject.width, subject.address_space
                    ))
                })?;
            let mut changed = false;
            let mut renamed_field = None;
            let outcome = edit_register(
                &mut self.device,
                subject.address,
                subject.width,
                &mut |register| {
                    if assertion.kind == "field-name"
                        && let Some(field) = subject.field
                    {
                        renamed_field = single_field_name(register, field)?.map(str::to_owned);
                    }
                    changed = apply_register_assertion(register, &subject, assertion)?;
                    Ok(())
                },
            )?;
            match outcome {
                RegisterEditOutcome::Edited => {}
                RegisterEditOutcome::ArrayInstance => {
                    return Err(Error::message(format!(
                        "reviewed assertion {:?} targets one expanded array instance; use a reusable array-wide base model or expand it before review",
                        assertion.id
                    )));
                }
                RegisterEditOutcome::NotFound => {
                    return Err(Error::message(format!(
                        "reviewed assertion {:?} could not locate physical register {:#010x}/{}",
                        assertion.id, subject.address, subject.width
                    )));
                }
            }
            summary.matched += 1;
            if changed {
                summary.changed += 1;
                if assertion.kind == "register-name" {
                    let new_identity = self
                        .register_identities()?
                        .get(&(subject.address, subject.width))
                        .cloned()
                        .ok_or_else(|| Error::message("renamed register disappeared"))?;
                    rewrite_review_entity_prefix(&mut self.review, &old_identity, &new_identity);
                } else if assertion.kind == "field-name"
                    && let Some(old_field) = renamed_field
                {
                    let new_field = string_value(assertion, "field name")?;
                    rewrite_review_entity_prefix(
                        &mut self.review,
                        &format!("{old_identity}.{old_field}"),
                        &format!("{old_identity}.{new_field}"),
                    );
                }
            }
        }
        model_validation::validate_device(&self.device)?;
        self.register_identities()?;
        Ok(summary)
    }

    pub fn validate_lints(&self, pack: &RegisterLintPack) -> Result<()> {
        pack.validate_device(&self.device)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RegisterSubject {
    address_space: String,
    address: u64,
    width: u32,
    field: Option<(u32, u32)>,
}

impl RegisterSubject {
    fn parse(value: &str) -> std::result::Result<Self, String> {
        let value = value
            .strip_prefix("mmio:")
            .ok_or_else(|| "expected mmio:<space>:0x<address>/<width>".to_owned())?;
        let (address_space, physical) = value
            .split_once(':')
            .ok_or_else(|| "missing MMIO address space".to_owned())?;
        if address_space.is_empty() {
            return Err("empty MMIO address space".to_owned());
        }
        let (physical, field) = physical
            .split_once('#')
            .map_or((physical, None), |(physical, field)| {
                (physical, Some(field))
            });
        let (address, width) = physical
            .split_once('/')
            .ok_or_else(|| "missing physical register width".to_owned())?;
        let address = address
            .strip_prefix("0x")
            .ok_or_else(|| "address must use a 0x prefix".to_owned())?;
        let address = u64::from_str_radix(address, 16)
            .map_err(|_| "invalid hexadecimal register address".to_owned())?;
        let width = width
            .parse::<u32>()
            .map_err(|_| "invalid register width".to_owned())?;
        if width == 0 {
            return Err("register width must be non-zero".to_owned());
        }
        let field = field
            .map(|field| {
                let field = field
                    .strip_prefix("bits:")
                    .ok_or_else(|| "field suffix must be #bits:<offset>/<width>".to_owned())?;
                let (offset, width) = field
                    .split_once('/')
                    .ok_or_else(|| "field suffix is missing width".to_owned())?;
                let offset = offset
                    .parse::<u32>()
                    .map_err(|_| "invalid field bit offset".to_owned())?;
                let width = width
                    .parse::<u32>()
                    .map_err(|_| "invalid field bit width".to_owned())?;
                if width == 0 || offset.checked_add(width).is_none_or(|end| end > 128) {
                    return Err("invalid field bit range".to_owned());
                }
                Ok((offset, width))
            })
            .transpose()?;
        Ok(Self {
            address_space: address_space.to_owned(),
            address,
            width,
            field,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RegisterEditOutcome {
    NotFound,
    Edited,
    ArrayInstance,
}

fn edit_register(
    device: &mut Device,
    address: u64,
    width: u32,
    edit: &mut impl FnMut(&mut RegisterInfo) -> Result<()>,
) -> Result<RegisterEditOutcome> {
    for peripheral in &mut device.peripherals {
        let outcome = match peripheral {
            MaybeArray::Single(peripheral) => edit_children(
                peripheral.base_address,
                0,
                merge_properties(
                    device.default_register_properties,
                    peripheral.default_register_properties,
                ),
                peripheral.registers.as_deref_mut().unwrap_or_default(),
                address,
                width,
                edit,
            )?,
            MaybeArray::Array(peripheral, dim) => {
                let inherited = merge_properties(
                    device.default_register_properties,
                    peripheral.default_register_properties,
                );
                let matches = (0..dim.dim).any(|index| {
                    contains_register(
                        peripheral.base_address + u64::from(index) * u64::from(dim.dim_increment),
                        0,
                        inherited,
                        peripheral.registers.as_deref().unwrap_or_default(),
                        address,
                        width,
                    )
                });
                if matches {
                    RegisterEditOutcome::ArrayInstance
                } else {
                    RegisterEditOutcome::NotFound
                }
            }
        };
        if outcome != RegisterEditOutcome::NotFound {
            return Ok(outcome);
        }
    }
    Ok(RegisterEditOutcome::NotFound)
}

fn edit_children(
    peripheral_base: u64,
    parent_offset: u64,
    inherited: RegisterProperties,
    children: &mut [RegisterCluster],
    address: u64,
    width: u32,
    edit: &mut impl FnMut(&mut RegisterInfo) -> Result<()>,
) -> Result<RegisterEditOutcome> {
    for child in children {
        let outcome = match child {
            RegisterCluster::Register(MaybeArray::Single(register)) => {
                let properties = merge_properties(inherited, register.properties);
                if physical_match(
                    peripheral_base,
                    parent_offset + u64::from(register.address_offset),
                    properties,
                    address,
                    width,
                ) {
                    edit(register)?;
                    RegisterEditOutcome::Edited
                } else {
                    RegisterEditOutcome::NotFound
                }
            }
            RegisterCluster::Register(MaybeArray::Array(register, dim)) => {
                let properties = merge_properties(inherited, register.properties);
                let matches = (0..dim.dim).any(|index| {
                    physical_match(
                        peripheral_base,
                        parent_offset
                            + u64::from(register.address_offset)
                            + u64::from(index) * u64::from(dim.dim_increment),
                        properties,
                        address,
                        width,
                    )
                });
                if matches {
                    RegisterEditOutcome::ArrayInstance
                } else {
                    RegisterEditOutcome::NotFound
                }
            }
            RegisterCluster::Cluster(MaybeArray::Single(cluster)) => edit_children(
                peripheral_base,
                parent_offset + u64::from(cluster.address_offset),
                merge_properties(inherited, cluster.default_register_properties),
                &mut cluster.children,
                address,
                width,
                edit,
            )?,
            RegisterCluster::Cluster(MaybeArray::Array(cluster, dim)) => {
                let properties = merge_properties(inherited, cluster.default_register_properties);
                let matches = (0..dim.dim).any(|index| {
                    contains_register(
                        peripheral_base,
                        parent_offset
                            + u64::from(cluster.address_offset)
                            + u64::from(index) * u64::from(dim.dim_increment),
                        properties,
                        &cluster.children,
                        address,
                        width,
                    )
                });
                if matches {
                    RegisterEditOutcome::ArrayInstance
                } else {
                    RegisterEditOutcome::NotFound
                }
            }
        };
        if outcome != RegisterEditOutcome::NotFound {
            return Ok(outcome);
        }
    }
    Ok(RegisterEditOutcome::NotFound)
}

fn contains_register(
    peripheral_base: u64,
    parent_offset: u64,
    inherited: RegisterProperties,
    children: &[RegisterCluster],
    address: u64,
    width: u32,
) -> bool {
    children.iter().any(|child| match child {
        RegisterCluster::Register(register) => match register {
            MaybeArray::Single(register) => physical_match(
                peripheral_base,
                parent_offset + u64::from(register.address_offset),
                merge_properties(inherited, register.properties),
                address,
                width,
            ),
            MaybeArray::Array(register, dim) => (0..dim.dim).any(|index| {
                physical_match(
                    peripheral_base,
                    parent_offset
                        + u64::from(register.address_offset)
                        + u64::from(index) * u64::from(dim.dim_increment),
                    merge_properties(inherited, register.properties),
                    address,
                    width,
                )
            }),
        },
        RegisterCluster::Cluster(cluster) => match cluster {
            MaybeArray::Single(cluster) => contains_register(
                peripheral_base,
                parent_offset + u64::from(cluster.address_offset),
                merge_properties(inherited, cluster.default_register_properties),
                &cluster.children,
                address,
                width,
            ),
            MaybeArray::Array(cluster, dim) => (0..dim.dim).any(|index| {
                contains_register(
                    peripheral_base,
                    parent_offset
                        + u64::from(cluster.address_offset)
                        + u64::from(index) * u64::from(dim.dim_increment),
                    merge_properties(inherited, cluster.default_register_properties),
                    &cluster.children,
                    address,
                    width,
                )
            }),
        },
    })
}

fn physical_match(
    peripheral_base: u64,
    offset: u64,
    properties: RegisterProperties,
    address: u64,
    width: u32,
) -> bool {
    peripheral_base.checked_add(offset) == Some(address) && properties.size == Some(width)
}

fn apply_register_assertion(
    register: &mut RegisterInfo,
    subject: &RegisterSubject,
    assertion: &EffectiveAssertion,
) -> Result<bool> {
    if let Some(field) = subject.field {
        return apply_field_assertion(register, field, assertion);
    }
    match assertion.kind.as_str() {
        "register-name" => replace_string(&mut register.name, assertion, "register name"),
        "register-description" => {
            replace_optional_string(&mut register.description, assertion, "register description")
        }
        "register-access" => {
            let access = parse_access(assertion)?;
            let changed = register.properties.access != Some(access);
            register.properties.access = Some(access);
            Ok(changed)
        }
        "hardware-write-semantics" => {
            let semantics = parse_write_semantics(assertion)?;
            let changed = register.modified_write_values != semantics;
            register.modified_write_values = semantics;
            Ok(changed)
        }
        kind => Err(Error::message(format!(
            "reviewed assertion {:?} kind {kind:?} requires a field subject",
            assertion.id
        ))),
    }
}

fn apply_field_assertion(
    register: &mut RegisterInfo,
    (offset, width): (u32, u32),
    assertion: &EffectiveAssertion,
) -> Result<bool> {
    let fields = register.fields.get_or_insert_with(Vec::new);
    let mut matched = None;
    for (index, field) in fields.iter().enumerate() {
        match field {
            MaybeArray::Single(field)
                if field.bit_offset() == offset && field.bit_width() == width =>
            {
                matched = Some(index);
                break;
            }
            MaybeArray::Array(field, dim)
                if (0..dim.dim).any(|index| {
                    field.bit_offset() + index * dim.dim_increment == offset
                        && field.bit_width() == width
                }) =>
            {
                return Err(Error::message(format!(
                    "reviewed assertion {:?} targets one expanded field-array instance",
                    assertion.id
                )));
            }
            _ => {}
        }
    }
    if matched.is_none() && assertion.kind == "field-name" {
        let name = string_value(assertion, "field name")?;
        let field = FieldInfo::builder()
            .name(name.to_owned())
            .bit_offset(offset)
            .bit_width(width)
            .build(ValidateLevel::Strict)?;
        fields.push(MaybeArray::Single(field));
        return Ok(true);
    }
    let Some(index) = matched else {
        return Err(Error::message(format!(
            "reviewed assertion {:?} targets absent field bits {offset}/{}; add field-name first",
            assertion.id, width
        )));
    };
    let MaybeArray::Single(field) = &mut fields[index] else {
        unreachable!("field arrays returned before mutation")
    };
    match assertion.kind.as_str() {
        "field-name" => replace_string(&mut field.name, assertion, "field name"),
        "field-description" => {
            replace_optional_string(&mut field.description, assertion, "field description")
        }
        "field-access" => {
            let access = parse_access(assertion)?;
            let changed = field.access != Some(access);
            field.access = Some(access);
            Ok(changed)
        }
        "field-write-semantics" => {
            let semantics = parse_write_semantics(assertion)?;
            let changed = field.modified_write_values != semantics;
            field.modified_write_values = semantics;
            Ok(changed)
        }
        kind => Err(Error::message(format!(
            "reviewed assertion {:?} kind {kind:?} requires a register subject",
            assertion.id
        ))),
    }
}

fn single_field_name(register: &RegisterInfo, (offset, width): (u32, u32)) -> Result<Option<&str>> {
    let Some(fields) = register.fields.as_ref() else {
        return Ok(None);
    };
    for field in fields {
        match field {
            MaybeArray::Single(field)
                if field.bit_offset() == offset && field.bit_width() == width =>
            {
                return Ok(Some(&field.name));
            }
            MaybeArray::Array(field, dim)
                if (0..dim.dim).any(|index| {
                    field.bit_offset() + index * dim.dim_increment == offset
                        && field.bit_width() == width
                }) =>
            {
                return Err(Error::message(
                    "reviewed field-name targets one expanded field-array instance",
                ));
            }
            _ => {}
        }
    }
    Ok(None)
}

fn replace_string(
    target: &mut String,
    assertion: &EffectiveAssertion,
    label: &str,
) -> Result<bool> {
    let value = string_value(assertion, label)?;
    let changed = target != value;
    target.replace_range(.., value);
    Ok(changed)
}

fn replace_optional_string(
    target: &mut Option<String>,
    assertion: &EffectiveAssertion,
    label: &str,
) -> Result<bool> {
    let value = string_value(assertion, label)?;
    let changed = target.as_deref() != Some(value);
    *target = Some(value.to_owned());
    Ok(changed)
}

fn string_value<'a>(assertion: &'a EffectiveAssertion, label: &str) -> Result<&'a str> {
    let AssertionValue::String(value) = &assertion.value else {
        return Err(Error::message(format!(
            "reviewed assertion {:?} {label} must be a string",
            assertion.id
        )));
    };
    if value.trim().is_empty() {
        return Err(Error::message(format!(
            "reviewed assertion {:?} {label} must not be empty",
            assertion.id
        )));
    }
    Ok(value)
}

fn parse_access(assertion: &EffectiveAssertion) -> Result<Access> {
    let value = string_value(assertion, "access")?;
    Access::parse_str(value).ok_or_else(|| {
        Error::message(format!(
            "reviewed assertion {:?} has unsupported access {value:?}",
            assertion.id
        ))
    })
}

fn parse_write_semantics(assertion: &EffectiveAssertion) -> Result<Option<ModifiedWriteValues>> {
    let value = string_value(assertion, "write semantics")?;
    let semantics = match value {
        "unknown" => return Ok(None),
        "w1c" | "one-to-clear" => ModifiedWriteValues::OneToClear,
        "w1s" | "one-to-set" => ModifiedWriteValues::OneToSet,
        "one-to-toggle" => ModifiedWriteValues::OneToToggle,
        "zero-to-clear" => ModifiedWriteValues::ZeroToClear,
        "zero-to-set" => ModifiedWriteValues::ZeroToSet,
        "zero-to-toggle" => ModifiedWriteValues::ZeroToToggle,
        "clear" => ModifiedWriteValues::Clear,
        "set" => ModifiedWriteValues::Set,
        "modify" => ModifiedWriteValues::Modify,
        _ => {
            return Err(Error::message(format!(
                "reviewed assertion {:?} has unsupported write semantics {value:?}",
                assertion.id
            )));
        }
    };
    Ok(Some(semantics))
}

fn is_register_assertion(kind: &str) -> bool {
    matches!(
        kind,
        "register-name"
            | "register-description"
            | "register-access"
            | "hardware-write-semantics"
            | "field-name"
            | "field-description"
            | "field-access"
            | "field-write-semantics"
    )
}

fn assertion_priority(kind: &str) -> u8 {
    match kind {
        "register-name" | "field-name" => 0,
        "register-description" | "field-description" => 1,
        "register-access" | "field-access" => 2,
        "hardware-write-semantics" | "field-write-semantics" => 3,
        _ => u8::MAX,
    }
}

fn rewrite_review_entity_prefix(review: &mut [ReviewAnnotation], old: &str, new: &str) {
    for annotation in review {
        if annotation.entity == old {
            annotation.entity = new.to_owned();
        } else if let Some(suffix) = annotation.entity.strip_prefix(old)
            && suffix.starts_with('.')
        {
            annotation.entity = format!("{new}{suffix}");
        }
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
            return Err(Error::message(format!(
                "empty or duplicate register review entity {:?}",
                annotation.entity
            )));
        }
        if !known_entities.contains(annotation.entity.as_str()) {
            return Err(Error::message(format!(
                "register review entity {:?} does not exist in the model",
                annotation.entity
            )));
        }
        let classification_fields = usize::from(annotation.provenance.is_some())
            + usize::from(annotation.accuracy.is_some())
            + usize::from(annotation.completeness.is_some());
        if classification_fields != 0 && classification_fields != 3 {
            return Err(Error::message(format!(
                "register review entity {:?} must define provenance, accuracy and completeness together",
                annotation.entity
            )));
        }
        let mut sources = BTreeSet::new();
        if annotation
            .sources
            .iter()
            .any(|source| source.is_empty() || !sources.insert(source))
        {
            return Err(Error::message(format!(
                "register review entity {:?} has empty or duplicate sources",
                annotation.entity
            )));
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
        return Err(Error::message(format!(
            "register model fragment must be a safe relative path: {value:?}"
        )));
    }
    Ok(())
}

fn validate_peripheral_names(peripherals: &[Peripheral]) -> Result<()> {
    let mut names = BTreeSet::new();
    for peripheral in peripherals {
        if !names.insert(peripheral.name.as_str()) {
            return Err(Error::message(format!(
                "duplicate register model peripheral {:?}",
                peripheral.name
            )));
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
                        Error::message(format!(
                            "register {path}.{} has no inherited size",
                            register.name
                        ))
                    })?;
                    let address = peripheral_base
                        .checked_add(parent_offset)
                        .and_then(|address| address.checked_add(u64::from(register.address_offset)))
                        .ok_or_else(|| Error::message("register model address overflow"))?;
                    let identity = format!("{path}.{}", register.name);
                    if let Some(previous) = output.insert((address, width), identity.clone()) {
                        return Err(Error::message(format!(
                            "register model aliases {previous} and {identity} at {address:#010x}/{width}; explicit alias support is required"
                        )));
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
                        .ok_or_else(|| Error::message("register cluster address overflow"))?;
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
            return Err(Error::message(format!(
                "register {identity} at {address:#010x} has unsupported width {width}"
            )));
        }
        let size_bytes = u64::from(*width).div_ceil(8);
        if address % size_bytes != 0 {
            return Err(Error::message(format!(
                "register {identity} at {address:#010x} is not aligned to {size_bytes} bytes"
            )));
        }
        let end = address
            .checked_add(size_bytes)
            .ok_or_else(|| Error::message("register model address overflow"))?;
        if let Some((previous_start, previous_end, previous_identity)) = previous
            && *address < previous_end
        {
            return Err(Error::message(format!(
                "physical register ranges overlap: {previous_identity} at \
                 {previous_start:#010x}..{previous_end:#010x} and {identity} at \
                 {address:#010x}..{end:#010x}; explicit alias support is required"
            )));
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
    use open_radio_vendor_review::ReviewPack;

    fn fixture_model(name: &str, modified_write_values: Option<&str>) -> (PathBuf, RegisterModel) {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-register-overlay-{}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("device.toml");
        fs::write(
            &manifest,
            "schema = 2\naddress-space = \"cpu\"\nfragments = [\"radio.toml\"]\n\n[device]\nname = \"device\"\nversion = \"1\"\ndescription = \"device\"\naddress-unit-bits = 8\nwidth = 32\n",
        )
        .unwrap();
        let semantics = modified_write_values.map_or_else(String::new, |value| {
            format!("modifiedWriteValues = \"{value}\"\n")
        });
        fs::write(
            directory.join("radio.toml"),
            format!(
                "schema = 2\n\n[[peripherals]]\nname = \"RADIO\"\nbaseAddress = 0x1000\n\n[[peripherals.registers]]\n[peripherals.registers.register]\nname = \"STATUS\"\naddressOffset = 0\nsize = 32\naccess = \"read-write\"\n{semantics}\n[[peripherals.registers.register.fields]]\nname = \"FLAG\"\nbitOffset = 3\nbitWidth = 1\n\n[[review]]\nentity = \"RADIO.STATUS\"\nsources = [\"fixture\"]\n\n[[review]]\nentity = \"RADIO.STATUS.FLAG\"\nsources = [\"fixture\"]\n"
            ),
        )
        .unwrap();
        let model = RegisterModel::load(&manifest).unwrap();
        (directory, model)
    }

    fn knowledge(assertions: &str) -> ReviewKnowledge {
        let pack = ReviewPack::from_toml(&format!(
            "schema = 1\nid = \"fixture.overlay\"\n[classification]\nprovenance = \"reviewed\"\naccuracy = \"exact\"\ncompleteness = \"partial\"\n{assertions}"
        ))
        .unwrap();
        ReviewKnowledge::merge([pack]).unwrap()
    }

    #[test]
    fn sparse_review_facts_override_names_fields_and_write_semantics() {
        let (directory, mut model) = fixture_model("apply", None);
        let knowledge = knowledge(
            r#"
[[assertions]]
id = "overlay.register.name"
subject = "mmio:cpu:0x1000/32"
kind = "register-name"
value = "EVENT_STATUS"
[[assertions.evidence]]
source = "FIXTURE"
locator = "register"

[[assertions]]
id = "overlay.field.name"
subject = "mmio:cpu:0x1000/32#bits:3/1"
kind = "field-name"
value = "PENDING"
[[assertions.evidence]]
source = "FIXTURE"
locator = "field"

[[assertions]]
id = "overlay.field.write"
subject = "mmio:cpu:0x1000/32#bits:3/1"
kind = "field-write-semantics"
value = "w1c"
[[assertions.evidence]]
source = "FIXTURE"
locator = "write"

[[assertions]]
id = "overlay.function"
subject = "function:radio_start"
kind = "function-name"
value = "RADIO_START"
[[assertions.evidence]]
source = "FIXTURE"
locator = "function"
"#,
        );

        let summary = model.apply_review_knowledge(&knowledge).unwrap();
        let (svd, _) = model.render_svd().unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(
            summary,
            ReviewApplicationSummary {
                matched: 3,
                changed: 3,
                ignored: 1,
            }
        );
        assert!(svd.contains("<name>EVENT_STATUS</name>"));
        assert!(svd.contains("<name>PENDING</name>"));
        assert!(svd.contains("<modifiedWriteValues>oneToClear</modifiedWriteValues>"));
        assert_eq!(
            model
                .review()
                .iter()
                .map(|annotation| annotation.entity.as_str())
                .collect::<Vec<_>>(),
            ["RADIO.EVENT_STATUS", "RADIO.EVENT_STATUS.PENDING"]
        );
    }

    #[test]
    fn explicit_unknown_clears_an_unreviewed_write_semantic() {
        let (directory, mut model) = fixture_model("clear", Some("oneToClear"));
        let knowledge = knowledge(
            r#"
[[assertions]]
id = "overlay.register.write"
subject = "mmio:cpu:0x1000/32"
kind = "hardware-write-semantics"
value = "unknown"
[[assertions.evidence]]
source = "FIXTURE"
locator = "write"
"#,
        );

        let summary = model.apply_review_knowledge(&knowledge).unwrap();
        let (svd, _) = model.render_svd().unwrap();
        fs::remove_dir_all(directory).unwrap();

        assert_eq!(summary.matched, 1);
        assert_eq!(summary.changed, 1);
        assert!(!svd.contains("modifiedWriteValues"));
    }

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

    #[test]
    fn malformed_manifest_error_retains_the_input_path() {
        let path = std::env::temp_dir().join(format!(
            "open-radio-register-model-malformed-{}.toml",
            std::process::id()
        ));
        fs::write(&path, "schema = [\n").unwrap();
        let error = RegisterModel::load(&path).unwrap_err();
        fs::remove_file(&path).unwrap();

        let (kind, reported, _, span) = error.manifest_diagnostic().unwrap();
        assert_eq!(kind, "register model manifest");
        assert_eq!(reported, path);
        assert!(span.is_some());
    }

    #[test]
    fn schema_error_retains_the_exact_value_span() {
        let path = std::env::temp_dir().join(format!(
            "open-radio-register-model-schema-{}.toml",
            std::process::id()
        ));
        let input = "schema = 1\nfragments = [\"peripherals.toml\"]\n\n[device]\nname = \"device\"\nversion = \"1\"\ndescription = \"device\"\naddress-unit-bits = 8\nwidth = 32\n";
        fs::write(&path, input).unwrap();
        let error = RegisterModel::load(&path).unwrap_err();
        fs::remove_file(&path).unwrap();

        let (_, _, _, span) = error.manifest_diagnostic().unwrap();
        assert_eq!(
            span.unwrap(),
            input.find('1').unwrap()..input.find('1').unwrap() + 1
        );
    }

    #[test]
    fn model_inputs_include_every_fragment() {
        let directory = std::env::temp_dir().join(format!(
            "open-radio-register-model-inputs-{}",
            std::process::id()
        ));
        let path = directory.join("device.toml");
        fs::create_dir_all(directory.join("peripherals")).unwrap();
        fs::write(
            &path,
            "schema = 2\nfragments = [\"peripherals/baseband.toml\", \"peripherals/agc.toml\"]\n\n[device]\nname = \"device\"\nversion = \"1\"\ndescription = \"device\"\naddress-unit-bits = 8\nwidth = 32\n",
        )
        .unwrap();

        let inputs = RegisterModel::input_paths(&path).unwrap();
        fs::remove_dir_all(&directory).unwrap();

        assert_eq!(
            inputs,
            vec![
                path,
                directory.join("peripherals/baseband.toml"),
                directory.join("peripherals/agc.toml"),
            ]
        );
    }
}
