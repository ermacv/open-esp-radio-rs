//! External trace and hint adapter. The envelope is strict; payloads retain the
//! producer's complete record, including provenance and unsupported attributes.
use super::*;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationDocument {
    schema_version: u32,
    artifact: String,
    applicability: Value,
    observations: Vec<Observation>,
    gaps: Vec<CoverageGap>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    /// Producer-stable locator, including sequence for dynamic events.
    id: String,
    subject: RegisterSubject,
    kind: ObservationKind,
    physical_width: Option<u32>,
    access_width: Option<u8>,
    name: Option<String>,
    semantics: Option<String>,
    function: Option<String>,
    mask: Option<u32>,
    payload: Value,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ObservationKind {
    TraceRead,
    TraceWrite,
    Hint,
    Declaration,
}

impl RegisterInventory {
    fn import_replay(
        &mut self,
        path: &Path,
        bytes: &[u8],
        declarations_only: bool,
        chip: &str,
        space: &str,
    ) -> Result<()> {
        let text =
            std::str::from_utf8(bytes).map_err(|error| crate::Error::invalid(error.to_string()))?;
        let replay = crate::artifacts::parse_replay_observations(text)?;
        if declarations_only {
            return Ok(());
        }
        let source = self.loaded(path, "execution-trace", bytes);
        let run = self.record(
            &source,
            "execution-source",
            &json!([replay.artifact.sha256, replay.manifest.sha256]),
            serde_json::from_slice(bytes)?,
        );
        self.gaps.insert(CoverageGap {source:run.clone(),scope:"trace".to_owned(),reason:"concrete execution covers only the recorded scenario; per-access PC and containing function are unknown".to_owned()});
        if let Err(error) = replay.validate_freshness() {
            let reason = error.to_string();
            for input in &mut self.sources {
                if input.digest.as_ref() == Some(&source) {
                    input.state = SourceState::Stale {
                        reason: reason.clone(),
                    };
                }
            }
            self.gaps.insert(CoverageGap {
                source: run.clone(),
                scope: "trace-freshness".to_owned(),
                reason,
            });
        }
        for phase in replay.phases {
            for observation in phase.register_observations {
                let evidence = self.record(
                    &source,
                    "trace-access",
                    &json!([
                        replay.artifact.sha256,
                        replay.manifest.sha256,
                        phase.name,
                        observation.sequence
                    ]),
                    serde_json::to_value(&observation)?,
                );
                for id in self.access_subjects(
                    chip,
                    space,
                    u64::from(observation.address),
                    u32::from(observation.width),
                ) {
                    let register = self.registers.get_mut(&id).expect("inserted subject");
                    let lane =
                        ((u64::from(observation.address) - register.subject.address) * 8) as u32;
                    register.evidence.extend([evidence.clone(), run.clone()]);
                    register.access_widths.insert(observation.width);
                    let bits = mask(u32::from(observation.width))
                        .checked_shl(lane)
                        .unwrap_or(0);
                    if observation.access == "read" {
                        register.coverage.transport_read |= bits;
                    } else {
                        register.coverage.transport_written |= bits;
                    }
                    Self::field(
                        register,
                        lane,
                        u32::from(observation.width),
                        None,
                        "opaque",
                        &evidence,
                    );
                }
            }
        }
        Ok(())
    }

    pub(super) fn import_observations(
        &mut self,
        path: &Path,
        declarations_only: bool,
        chip: &str,
        space: &str,
    ) -> Result<()> {
        let bytes = std::fs::read(path)?;
        let envelope: Value = serde_json::from_slice(&bytes)?;
        if envelope["command"].as_str() == Some("execute replay") {
            return self.import_replay(path, &bytes, declarations_only, chip, space);
        }
        let document: ObservationDocument = serde_json::from_value(envelope)?;
        if document.schema_version != 1 || document.artifact.is_empty() {
            return Err(crate::Error::invalid(
                "register observations require schema_version 1 and artifact identity",
            ));
        }
        let mut ids = BTreeSet::new();
        for observation in &document.observations {
            if observation.id.is_empty() || !ids.insert(&observation.id) {
                return Err(crate::Error::invalid(
                    "register observation IDs must be nonempty and unique within a source",
                ));
            }
            if observation.physical_width.is_some_and(|width| {
                width == 0
                    || observation
                        .subject
                        .address
                        .checked_add(u64::from(width).div_ceil(8))
                        .is_none()
            }) || observation.access_width.is_some_and(|width| {
                observation
                    .subject
                    .address
                    .checked_add(u64::from(width).div_ceil(8))
                    .is_none()
            }) || observation
                .access_width
                .is_some_and(|width| !matches!(width, 8 | 16 | 32))
            {
                return Err(crate::Error::invalid("invalid observation geometry"));
            }
            if matches!(
                observation.kind,
                ObservationKind::TraceRead | ObservationKind::TraceWrite
            ) && observation.access_width.is_none()
            {
                return Err(crate::Error::invalid(
                    "trace observations require access_width",
                ));
            }
        }
        let digest = self.loaded(path, "observations", &bytes);
        self.gaps.extend(document.gaps);
        // Declarations establish containment before trace lanes are attached.
        for observation in &document.observations {
            if let Some(width) = observation.physical_width {
                let evidence = self.record(
                    &digest,
                    "external-observation",
                    &json!([document.artifact, document.applicability, observation.id]),
                    serde_json::to_value(observation)?,
                );
                let id = self.subject(observation.subject.clone());
                let register = self.registers.get_mut(&id).expect("inserted subject");
                register.physical_width.insert(width, evidence.clone());
                register.evidence.insert(evidence);
            }
        }
        if declarations_only {
            return Ok(());
        }
        for observation in document.observations {
            let evidence = self.record(
                &digest,
                "external-observation",
                &json!([document.artifact, document.applicability, observation.id]),
                serde_json::to_value(&observation)?,
            );
            let subject = &observation.subject;
            let subjects = if subject.route == "mmio" && subject.bank.is_none() {
                self.access_subjects(
                    &subject.chip,
                    &subject.address_space,
                    subject.address,
                    u32::from(observation.access_width.unwrap_or(8)),
                )
            } else {
                vec![self.subject(subject.clone())]
            };
            for id in subjects {
                let register = self.registers.get_mut(&id).expect("inserted subject");
                register.evidence.insert(evidence.clone());
                if let Some(name) = &observation.name {
                    register.names.insert(name.clone(), evidence.clone());
                }
                if let Some(semantics) = &observation.semantics {
                    register
                        .semantics
                        .insert(semantics.clone(), evidence.clone());
                }
                if let Some(function) = &observation.function {
                    register.functions.insert(function.clone());
                }
                let lane = ((subject.address - register.subject.address) * 8) as u32;
                if let Some(width) = observation.access_width {
                    register.access_widths.insert(width);
                    Self::field(register, lane, u32::from(width), None, "opaque", &evidence);
                    let bits = mask(u32::from(width)).checked_shl(lane).unwrap_or(0);
                    match observation.kind {
                        ObservationKind::TraceRead => register.coverage.transport_read |= bits,
                        ObservationKind::TraceWrite => register.coverage.transport_written |= bits,
                        _ => (),
                    }
                }
                if let Some(bits) = observation.mask {
                    Self::bit_use_at(register, bits, lane, "candidate", &evidence);
                }
            }
        }
        Ok(())
    }
}
