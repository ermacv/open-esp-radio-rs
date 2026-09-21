//! Unified register queries. All projections retain links to complete evidence.
//!
//! Imported declarations and observed accesses are independent claims. This
//! module does not infer hardware semantics from software access patterns.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use open_radio_vendor_contracts::register_inventory::{
    CoverageGap, KnowledgeProperty, RegisterSubject, SourceState, bit_slices,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{ProjectSpec, Result};

mod hints;
mod observations;
mod query;
pub use query::{InventoryQuery, RegisterQuestion};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventorySource {
    pub path: String,
    pub kind: String,
    pub digest: Option<String>,
    pub evidence: BTreeSet<String>,
    pub state: SourceState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RegisterEvidence {
    pub id: String,
    pub kind: String,
    pub identity: Value,
    pub sources: BTreeSet<String>,
    /// Complete validated input record; no lossy secondary evidence DTO.
    pub payload: Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryField {
    pub id: String,
    pub offset: u32,
    pub width: u32,
    pub mask: Option<u32>,
    pub names: KnowledgeProperty<String>,
    pub kind: String,
    pub semantics: KnowledgeProperty<String>,
    pub evidence: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BitCoverage {
    pub transport_read: u32,
    pub transport_written: u32,
    pub consumed: u32,
    pub modified: u32,
    pub preserved: u32,
    pub described: u32,
    pub unobserved: Option<u32>,
    pub undescribed: Option<u32>,
    /// Half-open bit intervals cover arbitrary physical widths. None means
    /// the universe is unknown; an empty vector means no gap in this scope.
    pub unobserved_intervals: Option<Vec<(u64, u64)>>,
    pub undescribed_intervals: Option<Vec<(u64, u64)>>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InventoryRegister {
    pub id: String,
    pub subject: RegisterSubject,
    pub names: KnowledgeProperty<String>,
    pub physical_width: KnowledgeProperty<u32>,
    pub semantics: KnowledgeProperty<String>,
    pub access_widths: BTreeSet<u8>,
    pub functions: BTreeSet<String>,
    pub fields: BTreeMap<String, InventoryField>,
    pub evidence: BTreeSet<String>,
    pub coverage: BitCoverage,
}

impl InventoryRegister {
    pub fn width(&self) -> Option<u32> {
        match &self.physical_width {
            KnowledgeProperty::Known { claims } => claims.first().map(|claim| claim.value),
            _ => None,
        }
    }

    pub fn label(&self) -> String {
        if self.names.is_unknown() {
            format!("UNKNOWN@{:#010x}", self.subject.address)
        } else {
            self.names
                .values()
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ")
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct AddressCoverage {
    pub name: String,
    pub address_space: String,
    pub alias_of: Option<String>,
    pub evidence: BTreeSet<String>,
    pub start: u64,
    pub end_exclusive: u64,
    pub boundary: String,
    pub known_geometry: Vec<(u64, u64)>,
    pub observed_extents: Vec<(u64, u64)>,
    pub geometry_gaps: Vec<(u64, u64)>,
    pub observation_gaps: Vec<(u64, u64)>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RegisterInventory {
    pub schema_version: u32,
    pub sources: Vec<InventorySource>,
    pub registers: BTreeMap<String, InventoryRegister>,
    pub evidence: BTreeMap<String, RegisterEvidence>,
    pub gaps: BTreeSet<CoverageGap>,
    pub regions: Vec<AddressCoverage>,
    pub address_domains: BTreeSet<String>,
}

impl Default for RegisterInventory {
    fn default() -> Self {
        Self {
            schema_version: 1,
            sources: Vec::new(),
            registers: BTreeMap::new(),
            evidence: BTreeMap::new(),
            gaps: BTreeSet::new(),
            regions: Vec::new(),
            address_domains: BTreeSet::new(),
        }
    }
}

pub(crate) fn mask(width: u32) -> u32 {
    if width >= 32 {
        u32::MAX
    } else {
        (1_u32 << width) - 1
    }
}

fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| {
        value.as_str().and_then(|s| {
            s.strip_prefix("0x")
                .map_or_else(|| s.parse().ok(), |s| u64::from_str_radix(s, 16).ok())
        })
    })
}

fn words(value: &Value) -> impl Iterator<Item = String> + '_ {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
}

impl RegisterInventory {
    fn record(&mut self, source: &str, kind: &str, identity: &Value, payload: Value) -> String {
        let key = serde_json::to_vec(&(kind, identity, &payload)).expect("JSON values serialize");
        let id = format!("evidence:{:x}", Sha256::digest(key));
        let entry = self
            .evidence
            .entry(id.clone())
            .or_insert_with(|| RegisterEvidence {
                id: id.clone(),
                kind: kind.to_owned(),
                identity: identity.clone(),
                sources: BTreeSet::new(),
                payload,
            });
        entry.sources.insert(source.to_owned());
        if kind.ends_with("source") || kind == "analysis-run" {
            for input in &mut self.sources {
                if input.digest.as_deref() == Some(source) {
                    input.evidence.insert(id.clone());
                }
            }
        }
        id
    }

    fn location(&mut self, chip: &str, space: &str, address: u64) -> String {
        self.subject(RegisterSubject {
            chip: chip.to_owned(),
            address_space: space.to_owned(),
            route: "mmio".to_owned(),
            bank: None,
            address,
        })
    }

    fn subject(&mut self, subject: RegisterSubject) -> String {
        let id = subject.id();
        self.registers
            .entry(id.clone())
            .or_insert_with(|| InventoryRegister {
                id: id.clone(),
                subject,
                names: KnowledgeProperty::Unknown,
                physical_width: KnowledgeProperty::Unknown,
                semantics: KnowledgeProperty::Unknown,
                access_widths: BTreeSet::new(),
                functions: BTreeSet::new(),
                fields: BTreeMap::new(),
                evidence: BTreeSet::new(),
                coverage: BitCoverage::default(),
            });
        id
    }

    fn access_subjects(
        &mut self,
        chip: &str,
        space: &str,
        address: u64,
        width: u32,
    ) -> Vec<String> {
        let end = address.checked_add(u64::from(width).div_ceil(8));
        let ids: Vec<_> = self
            .registers
            .values()
            .filter(|register| {
                register.subject.chip == chip
                    && register.subject.address_space == space
                    && register.subject.route == "mmio"
                    && register.subject.bank.is_none()
                    && register.width().is_some_and(|bits| {
                        let limit = register
                            .subject
                            .address
                            .checked_add(u64::from(bits).div_ceil(8));
                        register.subject.address <= address
                            && end.zip(limit).is_some_and(|(end, limit)| end <= limit)
                    })
            })
            .map(|register| register.id.clone())
            .collect();
        if ids.is_empty() {
            vec![self.location(chip, space, address)]
        } else {
            ids
        }
    }

    fn field(
        register: &mut InventoryRegister,
        offset: u32,
        width: u32,
        name: Option<&str>,
        kind: &str,
        evidence: &str,
    ) {
        let id = format!("{}/bits/{offset}/{width}/{kind}", register.id);
        let bits = (width <= 32 && offset.checked_add(width).is_some_and(|end| end <= 32))
            .then(|| mask(width) << offset);
        let field = register
            .fields
            .entry(id.clone())
            .or_insert_with(|| InventoryField {
                id,
                offset,
                width,
                mask: bits,
                names: KnowledgeProperty::Unknown,
                kind: kind.to_owned(),
                semantics: KnowledgeProperty::Unknown,
                evidence: BTreeSet::new(),
            });
        if let Some(name) = name {
            field.names.insert(name.to_owned(), evidence.to_owned());
        }
        field.evidence.insert(evidence.to_owned());
        if kind == "declared" && offset < 32 {
            register.coverage.described |= mask(width.min(32 - offset)) << offset;
        }
    }

    #[cfg(test)]
    fn bit_use(register: &mut InventoryRegister, bits: u32, kind: &str, evidence: &str) {
        Self::bit_use_at(register, bits, 0, kind, evidence);
    }

    fn bit_use_at(
        register: &mut InventoryRegister,
        bits: u32,
        lane: u32,
        kind: &str,
        evidence: &str,
    ) {
        for (lsb, msb, _) in bit_slices(bits, 32) {
            Self::field(
                register,
                lane + u32::from(lsb),
                u32::from(msb - lsb + 1),
                None,
                kind,
                evidence,
            );
        }
    }

    fn unavailable(&mut self, path: &Path, kind: &str, error: Option<String>) {
        if let Ok(bytes) = std::fs::read(path) {
            let digest = self
                .sources
                .iter()
                .find(|source| source.path == path.display().to_string() && source.kind == kind)
                .and_then(|source| source.digest.clone())
                .unwrap_or_else(|| self.loaded(path, kind, &bytes));
            let payload = match String::from_utf8(bytes.clone()) {
                Ok(text) => json!({"text":text}),
                Err(_) => json!({"bytes":bytes}),
            };
            self.record(&digest, "opaque-source", &json!(digest), payload);
        }
        let path = path.display().to_string();
        let state = error
            .as_ref()
            .map_or(SourceState::Missing, |reason| SourceState::Failed {
                reason: reason.clone(),
            });
        if let Some(source) = self
            .sources
            .iter_mut()
            .find(|source| source.path == path && source.kind == kind)
        {
            source.state = state;
        } else {
            self.sources.push(InventorySource {
                path: path.clone(),
                kind: kind.to_owned(),
                digest: None,
                evidence: BTreeSet::new(),
                state,
            });
        }
        self.gaps.insert(CoverageGap {
            source: path,
            scope: kind.to_owned(),
            reason: error.unwrap_or_else(|| "configured source is missing".to_owned()),
        });
    }

    fn loaded(&mut self, path: &Path, kind: &str, bytes: &[u8]) -> String {
        let digest = format!("{:x}", Sha256::digest(bytes));
        self.sources.push(InventorySource {
            path: path.display().to_string(),
            kind: kind.to_owned(),
            digest: Some(digest.clone()),
            evidence: BTreeSet::new(),
            state: SourceState::Loaded,
        });
        digest
    }

    fn import_regions(
        &mut self,
        space: &str,
        source: &str,
        regions: Vec<open_esp_radio_register_model::PeripheralRegion>,
    ) {
        for region in regions {
            let evidence = self.record(
                source,
                "peripheral-region",
                &json!(source),
                serde_json::to_value(&region).expect("region serializes"),
            );
            self.regions.push(AddressCoverage {
                name: region.name,
                address_space: space.to_owned(),
                alias_of: None,
                evidence: BTreeSet::from([evidence]),
                start: region.start,
                end_exclusive: region.end_exclusive,
                boundary: "declared-address-block".to_owned(),
                known_geometry: Vec::new(),
                observed_extents: Vec::new(),
                geometry_gaps: Vec::new(),
                observation_gaps: Vec::new(),
            });
        }
    }

    fn import_geometry(
        &mut self,
        chip: &str,
        space: &str,
        source: &str,
        kind: &str,
        identity: &Value,
        geometries: Vec<open_esp_radio_register_model::RegisterGeometry>,
    ) {
        for geometry in geometries {
            let evidence = self.record(
                source,
                kind,
                identity,
                serde_json::to_value(&geometry).expect("geometry serializes"),
            );
            let id = self.location(chip, space, geometry.address);
            let register = self.registers.get_mut(&id).expect("inserted subject");
            register.names.insert(geometry.name, evidence.clone());
            if let Some(width) = geometry.width {
                register.physical_width.insert(width, evidence.clone());
            }
            register.evidence.insert(evidence.clone());
            if let Some(semantics) = geometry.declaration.modified_write_values {
                register
                    .semantics
                    .insert(format!("{semantics:?}"), evidence.clone());
            }
            for field in geometry.fields {
                Self::field(
                    register,
                    field.offset,
                    field.width,
                    Some(&field.name),
                    "declared",
                    &evidence,
                );
                if let Some(semantics) = field.declaration.modified_write_values {
                    let id = format!(
                        "{}/bits/{}/{}/declared",
                        register.id, field.offset, field.width
                    );
                    register
                        .fields
                        .get_mut(&id)
                        .expect("inserted field")
                        .semantics
                        .insert(format!("{semantics:?}"), evidence.clone());
                }
            }
        }
    }

    fn import_facts(&mut self, chip: &str, space: &str, path: &Path) -> Result<()> {
        let bytes = std::fs::read(path)?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| crate::Error::invalid(error.to_string()))?;
        crate::artifacts::parse_mmio_facts(text)?;
        let document: Value = serde_json::from_slice(&bytes)?;
        for fact in document["registers"].as_array().into_iter().flatten() {
            if !matches!(number(&fact["width"]), Some(8 | 16 | 32)) {
                return Err(crate::Error::invalid(
                    "discovery access width must be 8, 16 or 32",
                ));
            }
        }
        for observation in document["observations"].as_array().into_iter().flatten() {
            if matches!(
                observation["kind"].as_str(),
                Some("instruction-mmio" | "indexed-mmio" | "memory-classification-candidate")
            ) && !matches!(number(&observation["width"]), Some(8 | 16 | 32))
            {
                return Err(crate::Error::invalid(
                    "discovery observation has invalid access width",
                ));
            }
        }
        let digest = self.loaded(path, "discovery", &bytes);
        let mut run_payload = document.clone();
        run_payload
            .as_object_mut()
            .expect("validated document")
            .remove("registers");
        run_payload
            .as_object_mut()
            .expect("validated document")
            .remove("observations");
        let run = self.record(&digest, "analysis-run", &json!(digest), run_payload);
        self.gaps.insert(CoverageGap {source:run.clone(),scope:"static-analysis".to_owned(),reason:"best-effort discovery; coverage is restricted to this run's code selection, ranges and exploration budgets".to_owned()});
        for diagnostic in document["diagnostics"].as_array().into_iter().flatten() {
            self.gaps.insert(CoverageGap {
                source: run.clone(),
                scope: diagnostic["function"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_owned(),
                reason: diagnostic["message"]
                    .as_str()
                    .unwrap_or("unknown")
                    .to_owned(),
            });
        }
        for range in document["ranges"].as_array().into_iter().flatten() {
            if let (Some(start), Some(end)) =
                (number(&range["start"]), number(&range["end_exclusive"]))
            {
                self.regions.push(AddressCoverage {
                    address_space: space.to_owned(),
                    alias_of: None,
                    evidence: BTreeSet::from([run.clone()]),
                    name: range["name"].as_str().unwrap_or("unknown").to_owned(),
                    start,
                    end_exclusive: end,
                    boundary: "analysis-selection".to_owned(),
                    known_geometry: Vec::new(),
                    observed_extents: Vec::new(),
                    geometry_gaps: Vec::new(),
                    observation_gaps: Vec::new(),
                });
            }
        }
        for observation in document["observations"].as_array().into_iter().flatten() {
            if observation["kind"].as_str() == Some("code-coverage") {
                let evidence = self.record(
                    &digest,
                    "code-coverage",
                    &document["artifacts"],
                    observation.clone(),
                );
                self.gaps.insert(CoverageGap {
                    source: evidence,
                    scope: format!(
                        "executable:{}:{}",
                        observation["source"], observation["section"]
                    ),
                    reason: observation["reason"]
                        .as_str()
                        .unwrap_or("incomplete executable coverage")
                        .to_owned(),
                });
                continue;
            }

            if observation["kind"].as_str() == Some("call-context-frontier") {
                let evidence = self.record(
                    &digest,
                    "call-context-frontier",
                    &document["artifacts"],
                    observation.clone(),
                );
                self.gaps.insert(CoverageGap {
                    source: evidence,
                    scope: "interprocedural-discovery".to_owned(),
                    reason: observation["reason"]
                        .as_str()
                        .unwrap_or("incomplete call context")
                        .to_owned(),
                });
                continue;
            }
            let instruction = observation["kind"].as_str() == Some("instruction-mmio");
            let evidence = self.record(
                &digest,
                if instruction {
                    "instruction-access"
                } else {
                    "address-domain"
                },
                &document["artifacts"],
                observation.clone(),
            );
            if !instruction {
                self.address_domains.insert(evidence.clone());
            }
            let addresses = observation["addresses"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(number)
                .chain(number(&observation["address"]));
            for address in addresses {
                let width = number(&observation["width"]).unwrap_or(0) as u32;
                for id in self.access_subjects(chip, space, address, width) {
                    let register = self.registers.get_mut(&id).expect("inserted subject");
                    register.evidence.insert(evidence.clone());
                    register.access_widths.insert(width as u8);
                    if let Some(function) = observation["function"].as_str() {
                        register.functions.insert(function.to_owned());
                    }
                    let lane = ((address - register.subject.address) * 8) as u32;
                    Self::field(register, lane, width, None, "opaque", &evidence);
                    register
                        .functions
                        .extend(words(&observation["call_context"]["caller_path"]));
                    if instruction {
                        let bits = mask(width).checked_shl(lane).unwrap_or(0);
                        match observation["access"].as_str() {
                            Some("Read") => register.coverage.transport_read |= bits,
                            Some("Write") => {
                                register.coverage.transport_written |= bits;
                                register.coverage.modified |=
                                    (number(&observation["modified_mask"]).unwrap_or(0) as u32)
                                        .checked_shl(lane)
                                        .unwrap_or(0);
                                register.coverage.preserved |=
                                    (number(&observation["preserved_mask"]).unwrap_or(0) as u32)
                                        .checked_shl(lane)
                                        .unwrap_or(0);
                            }
                            _ => (),
                        }
                    }
                    if instruction && observation["access"].as_str() == Some("Write") {
                        let bits = number(&observation["modified_mask"]).unwrap_or(0) as u32;
                        Self::bit_use_at(register, bits, lane, "candidate", &evidence);
                    }
                }
            }
        }
        for fact in document["registers"].as_array().into_iter().flatten() {
            let address = number(&fact["address"])
                .ok_or("discovery register has no address")
                .map_err(crate::Error::invalid)?;
            let width = number(&fact["width"])
                .ok_or("discovery register has no width")
                .map_err(crate::Error::invalid)? as u32;
            let evidence = self.record(
                &digest,
                "discovery-access",
                &document["artifacts"],
                fact.clone(),
            );
            for id in self.access_subjects(chip, space, address, width) {
                let register = self.registers.get_mut(&id).expect("inserted subject");
                let lane = ((address - register.subject.address) * 8) as u32;
                register.evidence.insert(evidence.clone());
                register.evidence.insert(run.clone());
                register.access_widths.insert(width as u8);
                Self::field(register, lane, width, None, "opaque", &evidence);
                register
                    .functions
                    .extend(words(&fact["read_functions"]).chain(words(&fact["write_functions"])));
                if number(&fact["reads"]).unwrap_or(0) > 0 {
                    register.coverage.transport_read |= mask(width).checked_shl(lane).unwrap_or(0);
                }
                if number(&fact["writes"]).unwrap_or(0) > 0 {
                    register.coverage.transport_written |=
                        mask(width).checked_shl(lane).unwrap_or(0);
                }
                for pattern in fact["write_patterns"].as_array().into_iter().flatten() {
                    let modified = number(&pattern["modified_mask"]).unwrap_or(0) as u32;
                    let preserved = number(&pattern["preserved_mask"]).unwrap_or(0) as u32;
                    register.coverage.modified |= modified.checked_shl(lane).unwrap_or(0);
                    register.coverage.preserved |= preserved.checked_shl(lane).unwrap_or(0);
                    Self::bit_use_at(
                        register,
                        modified,
                        lane,
                        if modified == u32::MAX {
                            "opaque"
                        } else {
                            "candidate"
                        },
                        &evidence,
                    );
                }
            }
        }
        Ok(())
    }

    fn import_ir(&mut self, chip: &str, space: &str, path: &Path) -> Result<()> {
        let reader = crate::artifacts::LinkedIrReader::open(path)?;
        let manifest = std::fs::read(path.join("manifest.json"))?;
        let mut bundle_identity = BTreeMap::new();
        for input in crate::artifacts::bundle_files(path) {
            bundle_identity.insert(
                input
                    .file_name()
                    .expect("bundle member")
                    .to_string_lossy()
                    .to_string(),
                format!("{:x}", Sha256::digest(std::fs::read(&input)?)),
            );
        }
        let digest = self.loaded(path, "linked-ir", &serde_json::to_vec(&bundle_identity)?);
        let manifest: Value = serde_json::from_slice(&manifest)?;
        self.record(&digest, "analysis-run", &json!(digest), manifest.clone());
        let mut users = BTreeMap::<String, BTreeSet<String>>::new();
        let function_labels = reader.discovery_function_labels();
        // Discovery and traces can establish users whose address-parametric
        // effects have no entry in this profile's standalone register index.
        for register in self.registers.values() {
            for label in &register.functions {
                for identity in function_labels.get(label).into_iter().flatten() {
                    users
                        .entry(identity.clone())
                        .or_default()
                        .insert(register.id.clone());
                }
            }
        }

        for register in reader.read_registers()? {
            let payload = serde_json::to_value(&register)?;
            let evidence = self.record(
                &digest,
                "linked-register",
                &manifest["artifacts"],
                payload.clone(),
            );
            for id in self.access_subjects(
                chip,
                space,
                u64::from(register.address),
                u32::from(register.width),
            ) {
                let target = self.registers.get_mut(&id).expect("inserted subject");
                let lane = ((u64::from(register.address) - target.subject.address) * 8) as u32;
                target.access_widths.insert(register.width);
                target.functions.extend(register.functions.iter().cloned());
                for function in &register.functions {
                    users
                        .entry(function.clone())
                        .or_default()
                        .insert(id.clone());
                }
                target.evidence.insert(evidence.clone());
                Self::field(
                    target,
                    lane,
                    u32::from(register.width),
                    None,
                    "opaque",
                    &evidence,
                );
                if number(&payload["read_shapes"]).unwrap_or(0) > 0 {
                    target.coverage.transport_read |= mask(u32::from(register.width))
                        .checked_shl(lane)
                        .unwrap_or(0);
                }
                if number(&payload["write_shapes"]).unwrap_or(0) > 0 {
                    target.coverage.transport_written |= mask(u32::from(register.width))
                        .checked_shl(lane)
                        .unwrap_or(0);
                }
                for key in ["write_masks", "predicate_masks", "poll_masks"] {
                    for bits in payload[key]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(number)
                    {
                        let bits = bits as u32;
                        let projected = bits.checked_shl(lane).unwrap_or(0);
                        if key == "write_masks" {
                            target.coverage.modified |= projected;
                        } else {
                            target.coverage.consumed |= projected;
                        }
                        Self::bit_use_at(
                            target,
                            bits,
                            lane,
                            if bits == u32::MAX {
                                "opaque"
                            } else {
                                "candidate"
                            },
                            &evidence,
                        );
                    }
                }
                for field in &register.field_candidates {
                    Self::field(
                        target,
                        u32::from(field.least_significant_bit) + lane,
                        u32::from(field.most_significant_bit - field.least_significant_bit + 1),
                        None,
                        "candidate",
                        &evidence,
                    );
                }
            }
        }
        let mut frontier = std::collections::VecDeque::from_iter(users.keys().cloned());
        while let Some(callee) = frontier.pop_front() {
            let subjects = users[&callee].clone();
            for edge in reader.incoming_edges(&callee)? {
                let evidence = self.record(
                    &digest,
                    "call-reachability",
                    &manifest["artifacts"],
                    serde_json::to_value(&edge)?,
                );
                let caller_subjects = users.entry(edge.caller.clone()).or_default();
                let previous = caller_subjects.len();
                caller_subjects.extend(subjects.iter().cloned());
                if caller_subjects.len() != previous {
                    frontier.push_back(edge.caller.clone());
                }
                for id in &subjects {
                    let register = self.registers.get_mut(id).expect("indexed subject");
                    register.evidence.insert(evidence.clone());
                    register.functions.insert(edge.caller.clone());
                }
            }
        }
        let mut string_objects = Vec::new();
        reader.visit_data_objects(|object| {
            let subjects=object.xrefs.iter().filter_map(|xref|users.get(&xref.function)).flatten().cloned().collect::<BTreeSet<_>>();
            let declaration=serde_json::to_value(&object)?;
            let bytes=declaration["initializer_hex"].as_str().and_then(|hex| {
                if hex.len()%2!=0 {return None;}
                hex.as_bytes().chunks_exact(2).map(|pair|u8::from_str_radix(std::str::from_utf8(pair).ok()?,16).ok()).collect::<Option<Vec<_>>>()
            }).unwrap_or_default();
            let strings=bytes.split(|byte|*byte==0).filter_map(|part|std::str::from_utf8(part).ok()).filter(|text|text.len()>2 && text.chars().all(|c|!c.is_control() || matches!(c,'\n'|'\r'|'\t'))).collect::<Vec<_>>();
            let evidence=self.record(&digest,"data-cooccurrence-hint",&json!([object.artifact_sha256,object.locator,object.occurrence]),json!({"relation":"same-function-or-caller; not a value-flow proof","strings":strings,"object":declaration}));
            if let Some(address) = object.address.as_ref().and_then(|address| number(&json!(address))) {
                string_objects.push(hints::StringObject { artifact: object.artifact_sha256.clone(), address, bytes, evidence: evidence.clone() });
            }
            for id in subjects {self.registers.get_mut(&id).expect("indexed subject").evidence.insert(evidence.clone());}
            Ok(())
        })?;
        for (identity, subjects) in users {
            let Some(function) = reader.get_function_by_identity(&identity)? else {
                self.gaps.insert(CoverageGap {
                    source: digest.clone(),
                    scope: identity,
                    reason: "register index references a missing function".to_owned(),
                });
                continue;
            };
            let payload = serde_json::to_value(&function)?;
            let evidence = self.record(
                &digest,
                "function-use",
                &json!([
                    function.artifact_sha256,
                    function.locator,
                    function.occurrence
                ]),
                payload.clone(),
            );
            for key in [
                "decode_blockers",
                "direct_diagnostics",
                "reference_diagnostics",
                "call_graph_diagnostics",
            ] {
                for diagnostic in payload[key].as_array().into_iter().flatten() {
                    self.gaps.insert(CoverageGap {
                        source: evidence.clone(),
                        scope: identity.clone(),
                        reason: diagnostic.to_string(),
                    });
                }
            }
            let mut call_hints = Vec::new();
            for call in payload["calls"].as_array().into_iter().flatten() {
                for hint in hints::call_hints(call, &function.artifact_sha256, &string_objects) {
                    call_hints.push(self.record(
                        &digest,
                        "call-string-hint",
                        &json!([function.artifact_sha256, function.identity, call["site"]]),
                        hint,
                    ));
                }
            }
            for id in subjects {
                let register = self.registers.get_mut(&id).expect("indexed subject");
                register.evidence.extend(call_hints.iter().cloned());
                register.evidence.insert(evidence.clone());
                for call in payload["calls"].as_array().into_iter().flatten() {
                    for source in call["argument_bit_sources"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        let (Some(address), Some(bit)) =
                            (number(&source["address"]), number(&source["register_bit"]))
                        else {
                            continue;
                        };
                        if address < register.subject.address {
                            continue;
                        }
                        let lane = address - register.subject.address;
                        if lane >= u64::from(register.width().unwrap_or(32)).div_ceil(8) {
                            continue;
                        }
                        let bit = bit + lane * 8;
                        if bit < 32 {
                            register.coverage.consumed |= 1_u32 << bit;
                        }
                        if let Ok(bit) = u32::try_from(bit) {
                            Self::field(register, bit, 1, None, "argument-use", &evidence);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn finish(&mut self) {
        let old_derivations = self
            .evidence
            .values()
            .filter(|record| record.kind == "geometry-complement")
            .map(|record| record.id.clone())
            .collect::<BTreeSet<_>>();
        self.evidence.retain(|id, _| !old_derivations.contains(id));
        let mut complements = Vec::new();
        for register in self.registers.values_mut() {
            register.fields.retain(|_, field| field.kind != "unknown");
            register.evidence.retain(|id| !old_derivations.contains(id));
            register.coverage.unobserved = None;
            register.coverage.undescribed = None;
            register.coverage.unobserved_intervals = None;
            register.coverage.undescribed_intervals = None;
            if let Some(width) = register.width() {
                let declared = register
                    .fields
                    .values()
                    .filter(|field| field.kind == "declared")
                    .map(|field| {
                        (
                            u64::from(field.offset),
                            u64::from(field.offset) + u64::from(field.width),
                        )
                    })
                    .collect::<Vec<_>>();
                let observed = register
                    .evidence
                    .iter()
                    .filter_map(|id| self.evidence.get(id))
                    .filter_map(observed_access)
                    .filter_map(|(address, bits)| {
                        let lane = address
                            .checked_sub(register.subject.address)?
                            .checked_mul(8)?;
                        Some((lane, lane.checked_add(bits)?))
                    })
                    .collect::<Vec<_>>();
                let unknown = interval_gaps(0, u64::from(width), &declared);
                register.coverage.unobserved_intervals =
                    Some(interval_gaps(0, u64::from(width), &observed));
                register.coverage.undescribed_intervals = Some(unknown.clone());
                if width <= 32 {
                    register.coverage.unobserved = Some(
                        mask(width)
                            & !(register.coverage.transport_read
                                | register.coverage.transport_written),
                    );
                    register.coverage.undescribed =
                        Some(mask(width) & !register.coverage.described);
                }
                complements.push((register.id.clone(), unknown.clone(), json!({"subject":register.id,"physical_width":register.physical_width,"declared_intervals":declared,"unknown_intervals":unknown,"inputs":register.evidence,"rule":"known width minus declared fields; not reserved hardware"})));
            }
        }
        if !complements.is_empty() {
            let source = self.loaded(
                Path::new("derived:register-bit-complement"),
                "inference",
                b"register-bit-complement-v1",
            );
            for (id, intervals, payload) in complements {
                let evidence = self.record(&source, "geometry-complement", &json!(id), payload);
                let register = self.registers.get_mut(&id).expect("existing subject");
                register.evidence.insert(evidence.clone());
                for (first, end) in intervals {
                    Self::field(
                        register,
                        first as u32,
                        (end - first) as u32,
                        None,
                        "unknown",
                        &evidence,
                    );
                }
            }
        }
        for region in &mut self.regions {
            region.known_geometry = self
                .registers
                .values()
                .filter(|register| {
                    register.subject.address_space == region.address_space
                        && register.subject.route == "mmio"
                        && register.subject.bank.is_none()
                })
                .filter_map(|register| {
                    register.width().and_then(|width| {
                        Some((
                            register.subject.address,
                            register
                                .subject
                                .address
                                .checked_add(u64::from(width).div_ceil(8))?,
                        ))
                    })
                })
                .filter(|(start, end)| *start < region.end_exclusive && region.start < *end)
                .collect();
            region.observed_extents = self
                .registers
                .values()
                .filter(|register| {
                    register.subject.address_space == region.address_space
                        && register.subject.route == "mmio"
                        && register.subject.bank.is_none()
                })
                .flat_map(|register| register.evidence.iter())
                .filter_map(|id| self.evidence.get(id))
                .filter_map(|evidence| {
                    let (address, width) = observed_access(evidence)?;
                    Some((address, address.checked_add(width.div_ceil(8))?))
                })
                .filter(|(start, end)| *start < region.end_exclusive && region.start < *end)
                .collect();
            region.known_geometry.sort_unstable();
            region.known_geometry.dedup();
            region.observed_extents.sort_unstable();
            region.observed_extents.dedup();
            region.geometry_gaps =
                interval_gaps(region.start, region.end_exclusive, &region.known_geometry);
            region.observation_gaps =
                interval_gaps(region.start, region.end_exclusive, &region.observed_extents);
        }
        self.sources
            .sort_by(|a, b| (&a.path, &a.kind).cmp(&(&b.path, &b.kind)));
        self.sources.dedup();
        self.regions.sort();
        self.regions.dedup();
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(crate::Error::invalid(
                "register inventory requires schema_version 1",
            ));
        }
        fn property<T: Clone + Ord>(
            value: &KnowledgeProperty<T>,
            evidence: &BTreeMap<String, RegisterEvidence>,
        ) -> Result<()> {
            value.validate().map_err(crate::Error::invalid)?;
            if value
                .claims()
                .any(|claim| !evidence.contains_key(&claim.evidence))
            {
                return Err(crate::Error::invalid("dangling property claim evidence"));
            }
            Ok(())
        }
        for (id, register) in &self.registers {
            property(&register.names, &self.evidence)?;
            property(&register.physical_width, &self.evidence)?;
            property(&register.semantics, &self.evidence)?;
            for field in register.fields.values() {
                property(&field.names, &self.evidence)?;
                property(&field.semantics, &self.evidence)?;
            }
            if id != &register.id || id != &register.subject.id() {
                return Err(crate::Error::invalid(
                    "register inventory subject identity mismatch",
                ));
            }
            for evidence in register.evidence.iter().chain(
                register
                    .fields
                    .values()
                    .flat_map(|field| field.evidence.iter()),
            ) {
                if !self.evidence.contains_key(evidence) {
                    return Err(crate::Error::invalid(format!(
                        "dangling register evidence {evidence}"
                    )));
                }
            }
            if register
                .fields
                .values()
                .any(|field| field.width == 0 || field.offset.checked_add(field.width).is_none())
            {
                return Err(crate::Error::invalid("invalid register field geometry"));
            }
        }
        for id in &self.address_domains {
            if !self.evidence.contains_key(id) {
                return Err(crate::Error::invalid("dangling address-domain evidence"));
            }
        }
        Ok(())
    }

    /// All subjects containing an address; ambiguities are returned explicitly.
    pub fn at_address(&self, address: u64) -> Vec<&InventoryRegister> {
        self.registers
            .values()
            .filter(|register| {
                register.subject.address == address
                    || register.width().is_some_and(|width| {
                        register.subject.address < address
                            && address
                                < register
                                    .subject
                                    .address
                                    .saturating_add(u64::from(width).div_ceil(8))
                    })
            })
            .collect()
    }
}

pub(crate) fn load(project: &ProjectSpec, catalog: &crate::MmioMap) -> Result<RegisterInventory> {
    let mut output = RegisterInventory::default();
    let model = project.registers.as_ref().and_then(|paths| {
        if !paths.model.is_file() {
            output.unavailable(&paths.model, "model", None);
            return None;
        }
        match crate::registers::RegisterModel::load(&paths.model) {
            Ok(model) => Some(model),
            Err(error) => {
                output.unavailable(&paths.model, "model", Some(error.to_string()));
                None
            }
        }
    });
    let query_key = inventory_cache_key(project, catalog, model.as_ref())?;
    if let Some(bytes) = super::query_store::QueryStore::open(&project.manifest)?.get(&query_key)? {
        let cached: RegisterInventory = serde_json::from_slice(&bytes)?;
        cached.validate()?;
        return Ok(cached);
    }
    let chip = model.as_ref().map_or_else(
        || {
            if project.review_context.chips.len() == 1 {
                project.review_context.chips[0].as_str()
            } else {
                "unknown"
            }
        },
        |model| model.chip(),
    );
    let space = model.as_ref().map_or("cpu", |model| model.address_space());
    if let Some(model) = &model {
        let mut inputs = BTreeMap::new();
        for (path, text) in model.loaded_inputs() {
            let source = output.loaded(path, "model", text.as_bytes());
            output.record(
                &source,
                "model-source",
                &json!(source),
                json!({"toml":text}),
            );
            inputs.insert(path.display().to_string(), source);
        }
        let source = output.loaded(
            &project.registers.as_ref().expect("model workspace").model,
            "model-composition",
            &serde_json::to_vec(&inputs)?,
        );
        output.import_regions(space, &source, model.peripheral_regions()?);
        output.import_geometry(
            chip,
            space,
            &source,
            "model-geometry",
            &json!(inputs),
            model.register_geometry()?,
        );
        for assertion in model.reviewed_register_facts() {
            let evidence = output.record(
                &source,
                "reviewed-assertion",
                &json!(assertion.subject),
                serde_json::to_value(assertion)?,
            );
            let address = match &assertion.subject {
                open_radio_vendor_contracts::SemanticEntityId::Register { address, .. }
                | open_radio_vendor_contracts::SemanticEntityId::RegisterField {
                    address, ..
                } => *address,
                _ => continue,
            };
            let id = output.location(chip, space, address);
            output
                .registers
                .get_mut(&id)
                .expect("inserted subject")
                .evidence
                .insert(evidence);
        }
    }
    if let Some(paths) = &project.registers {
        for path in &paths.reviewed_knowledge {
            if !path.is_file() {
                output.unavailable(path, "reviewed-knowledge", None);
                continue;
            }
            let result = (|| -> Result<()> {
                let text = std::fs::read_to_string(path)?;
                let source = output.loaded(path, "reviewed-knowledge", text.as_bytes());
                output.record(
                    &source,
                    "reviewed-source",
                    &json!(source),
                    json!({"toml":text}),
                );
                let document: Value = toml_edit::de::from_str(&text)
                    .map_err(|error| crate::Error::invalid(error.to_string()))?;
                if document["schema"].as_u64() != Some(2) {
                    return Err(crate::Error::invalid(
                        "reviewed register inputs require schema 2",
                    ));
                }
                for assertion in document["assertions"].as_array().into_iter().flatten() {
                    let Some(subject) = assertion["subject"].as_str() else {
                        continue;
                    };
                    let subject: open_radio_vendor_contracts::SemanticEntityId = subject
                        .parse::<open_radio_vendor_contracts::SemanticEntityId>()
                        .map_err(|error| crate::Error::invalid(error.to_string()))?;
                    let (chip, space, address, field) = match subject {
                        open_radio_vendor_contracts::SemanticEntityId::Register {
                            chip,
                            address_space,
                            address,
                            ..
                        } => (chip, address_space, address, None),
                        open_radio_vendor_contracts::SemanticEntityId::RegisterField {
                            chip,
                            address_space,
                            address,
                            bit_offset,
                            bit_width,
                            ..
                        } => (chip, address_space, address, Some((bit_offset, bit_width))),
                        _ => continue,
                    };
                    let evidence =
                        output.record(&source, "reviewed-claim", &json!(source), assertion.clone());
                    let id = output.location(&chip, &space, address);
                    let register = output.registers.get_mut(&id).expect("inserted subject");
                    register.evidence.insert(evidence.clone());
                    if let Some((offset, width)) = field {
                        RegisterInventory::field(
                            register,
                            offset,
                            width,
                            None,
                            "reviewed-claim",
                            &evidence,
                        );
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                output.unavailable(path, "reviewed-knowledge", Some(error.to_string()));
            }
        }
        if !paths.reviewed_knowledge.is_empty() {
            match crate::registers::load_effective_register_model(paths) {
                Ok(model) => {
                    let source = output.loaded(
                        &paths.model,
                        "effective-model",
                        &serde_json::to_vec(
                            &output
                                .sources
                                .iter()
                                .map(|source| (&source.kind, &source.digest))
                                .collect::<BTreeSet<_>>(),
                        )?,
                    );
                    output.import_geometry(
                        chip,
                        space,
                        &source,
                        "reviewed-geometry",
                        &json!(source),
                        model.register_geometry()?,
                    );
                }
                Err(error) => {
                    output.unavailable(&paths.model, "effective-model", Some(error.to_string()))
                }
            }
        }
    }
    for path in &project.svd_paths {
        if !path.is_file() {
            output.unavailable(path, "svd", None);
            continue;
        }
        let result = (|| -> Result<()> {
            let xml = std::fs::read_to_string(path)?;
            let geometry = open_esp_radio_register_model::svd_geometry(&xml)?;
            let digest = output.loaded(path, "svd", xml.as_bytes());
            output.record(&digest, "svd-source", &json!(digest), json!({"xml":xml}));
            output.import_regions(
                space,
                &digest,
                open_esp_radio_register_model::svd_regions(&xml)?,
            );
            output.import_geometry(
                chip,
                space,
                &digest,
                "svd-geometry",
                &json!(digest),
                geometry,
            );
            Ok(())
        })();
        if let Err(error) = result {
            output.unavailable(path, "svd", Some(error.to_string()));
        }
    }
    if let Some(paths) = &project.registers {
        for declarations_only in [true, false] {
            for path in &paths.observations {
                if !path.is_file() {
                    output.unavailable(path, "observations", None);
                    continue;
                }
                if let Err(error) = output.import_observations(path, declarations_only, chip, space)
                {
                    output.unavailable(path, "observations", Some(error.to_string()));
                }
            }
        }
        if paths.facts.is_file() {
            if let Err(error) = output.import_facts(chip, space, &paths.facts) {
                output.unavailable(&paths.facts, "discovery", Some(error.to_string()));
            }
        } else {
            output.unavailable(&paths.facts, "discovery", None);
        }
        for path in &paths.review_ir_reports {
            if path.is_dir() {
                if let Err(error) = output.import_ir(chip, space, path) {
                    output.unavailable(path, "linked-ir", Some(error.to_string()));
                }
            } else {
                output.unavailable(path, "linked-ir", None);
            }
        }
    }
    if let Some(path) = &project.memory_map {
        let result = (|| -> Result<()> {
            let text = std::fs::read_to_string(path)?;
            let map = crate::MemoryMap::load(path)?;
            let source = output.loaded(path, "memory-map", text.as_bytes());
            let evidence =
                output.record(&source, "memory-map", &json!(source), json!({"toml":text}));
            for region in map
                .regions
                .iter()
                .filter(|region| region.kind == crate::memory_map::MemoryRegionKind::Mmio)
            {
                output.regions.push(AddressCoverage {
                    address_space: region.address_space.clone(),
                    alias_of: region.alias_of.clone(),
                    evidence: BTreeSet::from([evidence.clone()]),
                    name: region.name.clone(),
                    start: region.start,
                    end_exclusive: region.end,
                    boundary: "declared-mmio-region".to_owned(),
                    known_geometry: Vec::new(),
                    observed_extents: Vec::new(),
                    geometry_gaps: Vec::new(),
                    observation_gaps: Vec::new(),
                });
            }
            Ok(())
        })();
        if let Err(error) = result {
            output.unavailable(path, "memory-map", Some(error.to_string()));
        }
    } else {
        for region in &catalog.regions {
            output.regions.push(AddressCoverage {
                address_space: space.to_owned(),
                alias_of: None,
                evidence: BTreeSet::new(),
                name: region.name.clone(),
                start: u64::from(region.start),
                end_exclusive: u64::from(region.end),
                boundary: "analysis-selection".to_owned(),
                known_geometry: Vec::new(),
                observed_extents: Vec::new(),
                geometry_gaps: Vec::new(),
                observation_gaps: Vec::new(),
            });
        }
    }
    output.finish();
    output.validate()?;
    // Immutable query results use the existing SQLite/CAS store. Inputs stay
    // authoritative; a removed cache is rebuilt from their complete records.
    let bytes = serde_json::to_vec(&output)?;
    let fingerprint = format!("{:x}", Sha256::digest(&bytes));
    let mut store = super::query_store::QueryStore::open(&project.manifest)?;
    let mut dependencies = Vec::new();
    for evidence in output.evidence.values() {
        let bytes = serde_json::to_vec(evidence)?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let key = format!("register-evidence-v1:{digest}");
        store.put(&key, "register-evidence", &digest, &[], &bytes)?;
        dependencies.push(key);
    }
    store.put(
        &query_key,
        "register-inventory",
        &fingerprint,
        &dependencies,
        &bytes,
    )?;
    Ok(output)
}

fn observed_access(evidence: &RegisterEvidence) -> Option<(u64, u64)> {
    let payload = &evidence.payload;
    match evidence.kind.as_str() {
        "instruction-access" | "trace-access" => {
            Some((number(&payload["address"])?, number(&payload["width"])?))
        }
        "discovery-access"
            if ["reads", "writes"]
                .iter()
                .any(|key| number(&payload[key]).unwrap_or(0) > 0) =>
        {
            Some((number(&payload["address"])?, number(&payload["width"])?))
        }
        "linked-register"
            if ["read_shapes", "write_shapes", "poll_shapes"]
                .iter()
                .any(|key| number(&payload[key]).unwrap_or(0) > 0) =>
        {
            Some((number(&payload["address"])?, number(&payload["width"])?))
        }
        "external-observation"
            if matches!(payload["kind"].as_str(), Some("trace-read" | "trace-write")) =>
        {
            Some((
                number(&payload["subject"]["address"])?,
                number(&payload["access_width"])?,
            ))
        }
        _ => None,
    }
}

/// Content identities include every configured source (including absent files),
/// model fragments and every linked bundle member. No mtime-only freshness.
fn inventory_cache_key(
    project: &ProjectSpec,
    catalog: &crate::MmioMap,
    model: Option<&crate::registers::RegisterModel>,
) -> Result<String> {
    let mut paths = BTreeSet::new();
    paths.extend(project.svd_paths.iter().cloned());
    paths.extend(project.memory_map.iter().cloned());
    if let Some(model) = model {
        paths.extend(model.loaded_inputs().keys().cloned());
    }
    if let Some(registers) = &project.registers {
        paths.insert(registers.model.clone());
        paths.insert(registers.facts.clone());
        paths.extend(registers.reviewed_knowledge.iter().cloned());
        paths.extend(registers.observations.iter().cloned());
        for observation in &registers.observations {
            if let Ok(bytes) = std::fs::read(observation)
                && let Ok(document) = serde_json::from_slice::<Value>(&bytes)
                && document["command"].as_str() == Some("execute replay")
            {
                for key in ["manifest", "artifact"] {
                    if let Some(path) = document[key]["path"].as_str() {
                        paths.insert(path.into());
                    }
                }
            }
        }
        for bundle in &registers.review_ir_reports {
            paths.extend(crate::artifacts::bundle_files(bundle));
        }
    }
    let mut digest = Sha256::new();
    digest.update(format!(
        "register-inventory-query-v4:{project:?}:{catalog:?}"
    ));
    for path in paths {
        digest.update(serde_json::to_vec(&path)?);
        match std::fs::read(&path) {
            Ok(bytes) => {
                digest.update(b"present");
                digest.update(Sha256::digest(bytes));
            }
            Err(error) => {
                digest.update(format!("unavailable:{:?}:{error}", error.kind()));
            }
        }
    }
    Ok(format!(
        "register-inventory-query-v4:{:x}",
        digest.finalize()
    ))
}

/// Complement of covered byte extents, without assuming a register stride.
pub fn interval_gaps(start: u64, end: u64, covered: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut intervals = covered.to_vec();
    intervals.sort_unstable();
    let mut cursor = start;
    let mut gaps = Vec::new();
    for (first, last) in intervals {
        if last <= cursor || first >= end {
            continue;
        }
        if first > cursor {
            gaps.push((cursor, first.min(end)));
        }
        cursor = cursor.max(last).min(end);
    }
    if cursor < end {
        gaps.push((cursor, end));
    }
    gaps
}

#[cfg(test)]
mod tests;
