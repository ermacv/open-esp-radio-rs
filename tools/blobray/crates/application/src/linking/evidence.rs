//! Validate adapter claims against captured occurrences; no tool text is interpreted.
use super::*;
use std::io::{BufRead, BufReader, Seek};

pub(super) fn roots(
    outputs: &mut Outputs,
    found: &Collected,
    invocation: &LinkInvocation<'_>,
    control: &mut dyn RunControl,
) -> Result<(Vec<ResolvedRoot>, TemporaryFile)> {
    // Canonical order is extraction, placement, exit, independent of pipe polling.
    outputs.placements.rewind().map_err(storage_io)?;
    let mut block = [0; WORK_BLOCK];
    loop {
        control.checkpoint(1)?;
        let count = outputs.placements.read(&mut block).map_err(storage_io)?;
        if count == 0 {
            break;
        }
        outputs
            .observations
            .write_all(&block[..count])
            .map_err(storage_io)?;
    }
    outputs.placements = invocation.workspace.temporary()?;
    if outputs.exit_observations != 1 {
        return Err(Error::new(
            ErrorCode::LinkBlocked,
            "missing linker exit evidence",
        ));
    }
    write_control_message(
        &mut outputs.observations,
        &LinkObservationRecord {
            schema: 1,
            observation: LinkObservation::ToolExit {
                code: outputs.exit_code,
                signal: outputs.signal,
            },
        },
    )?;
    outputs.observations.write_all(b"\n").map_err(storage_io)?;
    outputs.observations.rewind().map_err(storage_io)?;
    let mut reader = BufReader::new(&mut outputs.observations);
    let mut proof = invocation.workspace.temporary()?;
    let mut addresses = vec![None; found.roots.len()];
    let mut placed = vec![false; found.members.len()];
    let mut extracted = vec![false; found.members.len()];
    let mut exit = false;
    let mut line = Vec::new();
    loop {
        control.checkpoint(1)?;
        line.clear();
        reader
            .by_ref()
            .take(65537)
            .read_until(b'\n', &mut line)
            .map_err(storage_io)?;
        if line.is_empty() {
            break;
        }
        if line.len() > 65536 {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "link observation exceeds 64 KiB",
            ));
        }
        control.bytes(line.len())?;
        let record: LinkObservationRecord =
            serde_json::from_slice(&line).map_err(|e| invalid(e.to_string()))?;
        if record.schema != 1 {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "unsupported linker observation",
            ));
        }
        let member = |object: &LinkObject| {
            found
                .members
                .iter()
                .position(|m| m.input == object.input && m.object == object.object)
                .ok_or_else(|| invalid("linker observation names an unknown occurrence"))
        };
        let span = |evidence: &LinkEvidenceSpan| -> Result<()> {
            let length = match evidence.source {
                LinkEvidenceSource::Map => outputs.map.metadata(),
                LinkEvidenceSource::Extraction => outputs.extraction.metadata(),
            }
            .map_err(storage_io)?
            .len();
            if evidence.length == 0
                || evidence
                    .offset
                    .checked_add(evidence.length)
                    .is_none_or(|end| end > length)
            {
                return Err(invalid("link evidence lies outside retained raw output"));
            }
            Ok(())
        };
        match record.observation {
            LinkObservation::SectionPlacement {
                object,
                section,
                address,
                size,
                evidence,
            } => {
                span(&evidence)?;
                if address.checked_add(size).is_none_or(|end| end > 1u64 << 32) {
                    return Err(invalid("link placement exceeds RV32"));
                }
                let index = member(&object)?;
                placed[index] |= size != 0;
                let m = &found.members[index];
                let mut exact = false;
                for (index, root) in found.roots.iter().enumerate() {
                    if root.selection.input == m.input
                        && root.selection.symbol.object == m.object
                        && root.section == section
                    {
                        if root.section_size != size || addresses[index].is_some() {
                            return Err(Error::new(
                                ErrorCode::LinkBlocked,
                                "root section placement is transformed or ambiguous",
                            ));
                        }
                        addresses[index] = Some(
                            address
                                .checked_add(root.offset)
                                .ok_or_else(|| invalid("root address overflow"))?,
                        );
                        exact = true;
                    }
                }
                write_control_message(
                    &mut proof,
                    &ImageMapping {
                        input: m.input,
                        object: m.object.clone(),
                        payload: m.payload.clone().unwrap(),
                        section,
                        address,
                        size,
                        exact,
                    },
                )?;
                proof.write_all(b"\n").map_err(storage_io)?;
            }
            LinkObservation::ArchiveExtraction {
                object,
                cause,
                referring,
                evidence,
            } => {
                span(&evidence)?;
                let index = member(&object)?;
                let alias = &found.members[index].alias;
                if !invocation
                    .inputs
                    .iter()
                    .any(|i| matches!(i, LinkInput::Archive(paths) if paths.contains(alias)))
                    || extracted[index]
                {
                    return Err(invalid("invalid or duplicate archive extraction"));
                }
                if cause
                    .as_ref()
                    .is_none_or(|s| s.is_empty() || s.len() > 4096)
                {
                    return Err(invalid("missing archive extraction cause"));
                }
                if let Some(referring) = referring {
                    member(&referring)?;
                }
                extracted[index] = true;
            }
            LinkObservation::ToolExit { code, signal } => {
                if exit
                    || code != Some(0)
                    || signal.is_some()
                    || code != outputs.exit_code
                    || signal != outputs.signal
                {
                    return Err(invalid("invalid linker exit observation"));
                }
                exit = true;
            }
        }
    }
    if !exit {
        return Err(Error::new(
            ErrorCode::LinkBlocked,
            "missing linker exit evidence",
        ));
    }
    for (index, m) in found.members.iter().enumerate() {
        let lazy = invocation
            .inputs
            .iter()
            .any(|i| matches!(i, LinkInput::Archive(paths) if paths.contains(&m.alias)));
        if placed[index] && lazy && !extracted[index] {
            return Err(Error::new(
                ErrorCode::LinkBlocked,
                "missing archive extraction evidence",
            ));
        }
    }
    let result = found
        .roots
        .iter()
        .zip(addresses)
        .map(|(root, address)| {
            Ok(ResolvedRoot {
                selection: root.selection.clone(),
                name: root.name.clone(),
                size: root.size,
                address: address.ok_or_else(|| {
                    Error::new(
                        ErrorCode::LinkBlocked,
                        "no exact linker provenance for a requested root",
                    )
                })?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((result, proof))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn validate(mut change: impl FnMut(&mut Vec<LinkObservation>)) -> Result<Vec<ResolvedRoot>> {
        let dir = tempfile::tempdir().unwrap();
        let disk = TemporaryBudget::new(4 * 1024 * 1024, None).unwrap();
        let workspace = LinkWorkspace {
            directory: dir.path(),
            disk: &disk,
            elf_limit: 65536,
        };
        let id = ArtifactId::of_bytes(b"captured source");
        let object = ObjectId {
            artifact: id.clone(),
            location: ObjectLocation::Standalone,
        };
        let occurrence = LinkObject {
            input: 7,
            object: object.clone(),
        };
        let selected = EntrySelection {
            input: 7,
            symbol: SymbolId {
                object: object.clone(),
                table: SymbolTableKind::Static,
                table_section: 1,
                index: 1,
            },
        };
        let found = Collected {
            project: id.as_str().parse().unwrap(),
            blockers: vec![],
            members: vec![Member {
                input: 7,
                object,
                payload: Some(id.clone()),
                source: id.clone(),
                alias: "root.o".into(),
                elf: true,
            }],
            roots: vec![blobray_artifacts::LinkRootFacts {
                selection: selected,
                name: b"root".to_vec(),
                section: b"code".to_vec(),
                section_size: 16,
                offset: 4,
                size: 8,
            }],
        };
        let identity = LinkerIdentity {
            implementation: "test-capability".into(),
            version: "future".into(),
            executable: id,
        };
        let invocation = LinkInvocation {
            executable: Path::new("unused"),
            identity: &identity,
            contract: LinkerContract::ElfAnalysisLinkV1,
            workspace: &workspace,
            entry: b"root",
            roots: vec![],
            forced: vec!["root.o".into()],
            inputs: vec![],
            members: vec![LinkMember {
                alias: "root.o".into(),
                occurrence: occurrence.clone(),
            }],
            layout: ImageLayout {
                code: ImageRegion {
                    start: 0x1000,
                    length: 4096,
                },
                data: ImageRegion {
                    start: 0x2000,
                    length: 4096,
                },
            },
            definitions: vec![],
        };
        let mut outputs = Outputs::new(&workspace)?;
        outputs
            .map
            .write_all(b"opaque tool dialect; application cannot parse this")
            .unwrap();
        let mut records = vec![
            LinkObservation::SectionPlacement {
                object: occurrence,
                section: b"code".to_vec(),
                address: 0x1000,
                size: 16,
                evidence: LinkEvidenceSpan {
                    source: LinkEvidenceSource::Map,
                    offset: 0,
                    length: 6,
                },
            },
            LinkObservation::ToolExit {
                code: Some(0),
                signal: None,
            },
        ];
        change(&mut records);
        outputs.exited(Some(0), None);
        for record in records {
            outputs.observe(record, &mut || Ok(()))?;
        }
        roots(&mut outputs, &found, &invocation, &mut || Ok(())).map(|(r, _)| r)
    }
    #[test]
    fn roots_use_typed_claims_and_captured_offsets_not_a_tool_dialect() {
        let roots = validate(|_| {}).unwrap();
        assert_eq!(roots[0].address, 0x1004);
        assert_eq!(roots[0].selection.input, 7);
    }
    #[test]
    fn missing_ambiguous_transformed_foreign_or_unbacked_claims_fail_closed() {
        assert!(
            validate(|r| {
                r.remove(0);
            })
            .is_err()
        );
        assert!(
            validate(|r| {
                r.insert(0, r[0].clone());
            })
            .is_err()
        );
        assert!(
            validate(|r| {
                if let LinkObservation::SectionPlacement { size, .. } = &mut r[0] {
                    *size = 12;
                }
            })
            .is_err()
        );
        assert!(
            validate(|r| {
                if let LinkObservation::SectionPlacement { object, .. } = &mut r[0] {
                    object.input = 8;
                }
            })
            .is_err()
        );
        assert!(
            validate(|r| {
                if let LinkObservation::SectionPlacement { evidence, .. } = &mut r[0] {
                    evidence.length = 1000;
                }
            })
            .is_err()
        );
        assert!(
            validate(|r| {
                r.pop();
            })
            .is_err()
        );
    }
    #[test]
    fn forced_member_cannot_impersonate_archive_extraction() {
        assert!(
            validate(|r| {
                if let LinkObservation::SectionPlacement {
                    object, evidence, ..
                } = r[0].clone()
                {
                    r.push(LinkObservation::ArchiveExtraction {
                        object,
                        evidence,
                        cause: Some(b"root".to_vec()),
                        referring: None,
                    });
                }
            })
            .is_err()
        );
    }
}
