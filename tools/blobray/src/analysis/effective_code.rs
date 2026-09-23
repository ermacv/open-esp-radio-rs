//! One project-level catalog combining ELF symbols with reviewed code ranges.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use crate::{
    Result, artifact, artifacts::symbol_inventory::load_code_boundary_facts,
    code_workspace::CodeWorkspace, project::ProjectSpec,
};

#[derive(Clone, Debug, Default)]
pub(crate) struct EffectiveCodeCatalog {
    source_digests: BTreeMap<String, BTreeSet<String>>,
    reviewed: BTreeMap<String, Vec<artifact::ReviewedCodeRange>>,
}

pub(crate) struct EffectiveCodeSymbols {
    pub(crate) symbols: Vec<artifact::ArtifactSymbolDefinition>,
    pub(crate) reviewed_boundaries: usize,
}

impl EffectiveCodeCatalog {
    pub(crate) fn load(project: &ProjectSpec) -> Result<Self> {
        let Some(paths) = &project.code else {
            return Ok(Self::default());
        };
        let inventory = &project
            .symbol_inventory
            .as_ref()
            .ok_or_else(|| crate::Error::invalid("[code] requires [analysis.symbols]"))?
            .output;
        if !inventory.is_file() {
            return Err(crate::Error::invalid(format!(
                "code-boundary facts are missing at {}; run `blobray advanced symbols inventory --project PATH` first",
                inventory.display(),
            )));
        }
        let facts = load_code_boundary_facts(inventory)?;
        let workspace = CodeWorkspace::load(&facts, &paths.pack, &project.id)?;
        let source_digests = facts.inputs.iter().fold(
            BTreeMap::<String, BTreeSet<String>>::new(),
            |mut inputs, input| {
                inputs
                    .entry(input.source.clone())
                    .or_default()
                    .insert(input.artifact_sha256.clone());
                inputs
            },
        );
        let mut reviewed = BTreeMap::<String, Vec<_>>::new();
        for entry in workspace.accepted() {
            reviewed
                .entry(entry.review.artifact_sha256.clone())
                .or_default()
                .push(artifact::ReviewedCodeRange {
                    member: entry.review.member.clone(),
                    section: entry.review.section.clone(),
                    name: entry.review.name.clone().expect("accepted boundary name"),
                    start_offset: entry.review.entry_offset,
                    end_offset: entry.review.end_exclusive_offset,
                });
        }
        Ok(Self {
            source_digests,
            reviewed,
        })
    }

    pub(crate) fn reviewed_ranges(
        &self,
        source: &str,
        path: &Path,
        capture: &artifact::CapturedArtifact<'_>,
    ) -> Result<Vec<artifact::ReviewedCodeRange>> {
        let Some(expected_digests) = self.source_digests.get(source) else {
            return Ok(Vec::new());
        };
        let digest = capture.sha256();
        if !expected_digests.contains(digest) {
            return Err(crate::Error::invalid(format!(
                "artifact {} for source {source:?} does not match any reviewed code-boundary SHA-256 guard (actual {digest})",
                path.display()
            )));
        }
        let Some(ranges) = self.reviewed.get(digest) else {
            return Ok(Vec::new());
        };
        Ok(ranges.clone())
    }

    pub(crate) fn load_symbols(
        &self,
        source: &str,
        path: &Path,
        capture: &artifact::CapturedArtifact<'_>,
        prefix: &str,
        selection: artifact::CodeSymbolSelection,
    ) -> Result<EffectiveCodeSymbols> {
        let mut symbols = capture
            .code_symbols(prefix, selection)?
            .into_iter()
            .map(|symbol| symbol.definition.clone())
            .collect::<Vec<_>>();
        let ranges = self
            .reviewed_ranges(source, path, capture)?
            .into_iter()
            .filter(|range| range.name.starts_with(prefix))
            .collect::<Vec<_>>();
        let reviewed_boundaries = ranges.len();
        symbols.extend(capture.reviewed_code_ranges(&ranges)?);
        symbols.sort_by(|left, right| {
            (&left.member, &left.name, left.address).cmp(&(
                &right.member,
                &right.name,
                right.address,
            ))
        });
        Ok(EffectiveCodeSymbols {
            symbols,
            reviewed_boundaries,
        })
    }
}
