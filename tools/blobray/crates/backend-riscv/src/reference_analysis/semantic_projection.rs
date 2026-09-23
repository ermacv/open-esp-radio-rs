//! Proof boundary for transferring an exact reviewed function annotation.
//!
//! An archive association is a candidate, not linker or body-equivalence
//! evidence. Only this module can admit a candidate into the semantic view.
//! Rejected candidates remain inspectable; they never suppress body analysis.

use std::collections::BTreeMap;

use rv_asm::{Inst, Reg};

use crate::{DirectSemanticFunctionSpec, RiscvSummaryHooks, artifact};

use super::ReferenceSymbolKey;

/// Why an origin association cannot authorize an exact semantic annotation.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SemanticProjectionGap {
    /// Projection requires a relocatable origin and a resolved destination.
    InvalidAddressDomains,
    /// Empty bodies cannot establish equivalence.
    EmptyBody,
    /// Relocations/relaxation require a verified transformation receipt.
    RelocationProofRequired,
    /// The associated definitions contain different instruction bytes.
    DifferentBody,
    /// The current decoder cannot prove the instruction semantics.
    UnsupportedInstruction { reason: String },
    /// A PC-dependent instruction cannot be justified by byte identity alone.
    PositionDependent { offset: u64 },
    /// Even a local jump must land on an instruction boundary.
    InvalidControlFlow { offset: u64 },
    /// The declared body falls through into bytes whose identity is unknown.
    OpenBodyBoundary,
    /// A rebase proof does not cover unresolved direct calls or escaping PCs.
    CallProofRequired { offset: u64 },
    /// Multiple applicable reviewed contracts disagree.
    ConflictingSemantics,
}

/// A candidate's proof state. Values are observations, not authority tokens.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum SemanticProjectionStatus {
    /// Complete byte identity with no relocation or observable PC dependency.
    VerifiedPositionIndependentBody,
    /// Kept for investigation, without granting reviewed semantic authority.
    Unverified { gap: SemanticProjectionGap },
}

#[derive(Clone, Debug)]
struct Candidate {
    origin: artifact::ArtifactSymbolDefinition,
    linked: artifact::ArtifactSymbolDefinition,
    semantic: &'static DirectSemanticFunctionSpec,
    status: SemanticProjectionStatus,
}

/// Resolver-owned proof results, bound to the exact definitions inspected.
///
/// The map and entries are private: callers cannot inject a semantic spec or
/// deserialize a purported proof. The selected provider authenticates the raw
/// body; the backend independently checks whether transfer is justified.
///
/// ```compile_fail
/// use open_radio_vendor_backend_riscv::reference_analysis::SemanticProjectionCatalog;
/// let forged = SemanticProjectionCatalog { candidates: Default::default() };
/// ```
#[derive(Debug, Default)]
pub struct SemanticProjectionCatalog {
    candidates: BTreeMap<ReferenceSymbolKey, Vec<Candidate>>,
}

fn key(symbol: &artifact::ArtifactSymbolDefinition) -> ReferenceSymbolKey {
    symbol.identity.clone()
}

fn same_definition(
    left: &artifact::ArtifactSymbolDefinition,
    right: &artifact::ArtifactSymbolDefinition,
) -> bool {
    key(left) == key(right)
        && left.member == right.member
        && left.name == right.name
        && left.address == right.address
        && left.bytes == right.bytes
        && left.relocations == right.relocations
        && left.addresses_resolved == right.addresses_resolved
        && left.memory_regions == right.memory_regions
}

impl SemanticProjectionCatalog {
    /// Authenticate a raw candidate and retain its verified or unknown state.
    ///
    /// `None` means that the provider has no reviewed annotation for this
    /// origin; the separate origin catalog still owns that association.
    pub fn observe(
        &mut self,
        origin: &artifact::ArtifactSymbolDefinition,
        linked: &artifact::ArtifactSymbolDefinition,
        hooks: &RiscvSummaryHooks,
    ) -> Option<SemanticProjectionStatus> {
        let semantic = (hooks.direct_semantic)(origin)?;
        let status = match verify_position_independent_body(origin, linked) {
            Ok(()) => SemanticProjectionStatus::VerifiedPositionIndependentBody,
            Err(gap) => SemanticProjectionStatus::Unverified { gap },
        };
        let entries = self.candidates.entry(key(linked)).or_default();
        if !entries.iter().any(|candidate| {
            same_definition(&candidate.origin, origin)
                && same_definition(&candidate.linked, linked)
                && candidate.semantic == semantic
        }) {
            entries.push(Candidate {
                origin: origin.clone(),
                linked: linked.clone(),
                semantic,
                status: status.clone(),
            });
        }
        Some(status)
    }

    /// Return an annotation only for the exact inspected body and one contract.
    pub fn semantic(
        &self,
        linked: &artifact::ArtifactSymbolDefinition,
    ) -> Option<&'static DirectSemanticFunctionSpec> {
        let mut verified = self.matching(linked).filter(|candidate| {
            candidate.status == SemanticProjectionStatus::VerifiedPositionIndependentBody
        });
        let first = verified.next()?.semantic;
        verified
            .all(|candidate| candidate.semantic == first)
            .then_some(first)
    }

    /// Unverified candidates and conflicting verified contracts remain visible.
    pub fn gaps(&self, linked: &artifact::ArtifactSymbolDefinition) -> Vec<SemanticProjectionGap> {
        let mut gaps = Vec::new();
        let mut verified = None;
        for candidate in self.matching(linked) {
            let gap = match &candidate.status {
                SemanticProjectionStatus::Unverified { gap } => Some(gap.clone()),
                SemanticProjectionStatus::VerifiedPositionIndependentBody => {
                    let conflict = verified.is_some_and(|previous| previous != candidate.semantic);
                    verified = Some(candidate.semantic);
                    conflict.then_some(SemanticProjectionGap::ConflictingSemantics)
                }
            };
            if let Some(gap) = gap
                && !gaps.contains(&gap)
            {
                gaps.push(gap);
            }
        }
        gaps
    }

    fn matching<'a>(
        &'a self,
        linked: &'a artifact::ArtifactSymbolDefinition,
    ) -> impl Iterator<Item = &'a Candidate> {
        self.candidates
            .get(&key(linked))
            .into_iter()
            .flatten()
            .filter(move |candidate| same_definition(&candidate.linked, linked))
    }
}

/// This proof deliberately has a narrow, explicit claim. Relocation-bearing
/// and relaxed bodies need a different verifier with captured link evidence;
/// instruction-shape matching is never substituted for that evidence.
fn verify_position_independent_body(
    origin: &artifact::ArtifactSymbolDefinition,
    linked: &artifact::ArtifactSymbolDefinition,
) -> Result<(), SemanticProjectionGap> {
    use SemanticProjectionGap as Gap;
    let valid_range = |symbol: &artifact::ArtifactSymbolDefinition| {
        symbol.address.is_multiple_of(2)
            && symbol
                .address
                .checked_add(symbol.bytes.len() as u64)
                .is_some_and(|end| end <= 1_u64 << 32)
    };
    if origin.addresses_resolved
        || !linked.addresses_resolved
        || !valid_range(origin)
        || !valid_range(linked)
    {
        return Err(Gap::InvalidAddressDomains);
    }
    if origin.bytes.is_empty() || linked.bytes.is_empty() {
        return Err(Gap::EmptyBody);
    }
    if !origin.relocations.is_empty() || !linked.relocations.is_empty() {
        return Err(Gap::RelocationProofRequired);
    }
    if origin.bytes != linked.bytes {
        return Err(Gap::DifferentBody);
    }
    let instructions =
        artifact::decode_symbol(origin).map_err(|error| Gap::UnsupportedInstruction {
            reason: error.to_string(),
        })?;
    for instruction in &instructions {
        let offset = instruction.address - origin.address;
        let relative_target = match instruction.instruction {
            Inst::Auipc { .. } | Inst::Ecall | Inst::Ebreak => {
                return Err(Gap::PositionDependent { offset });
            }
            Inst::Jal {
                dest,
                offset: displacement,
            } if dest == Reg::ZERO => Some(displacement.as_i32()),
            Inst::Jal { .. } => return Err(Gap::CallProofRequired { offset }),
            Inst::Jalr { dest, .. } if dest != Reg::ZERO => {
                return Err(Gap::CallProofRequired { offset });
            }
            Inst::Jalr { .. } => None,
            Inst::Beq { offset, .. }
            | Inst::Bne { offset, .. }
            | Inst::Blt { offset, .. }
            | Inst::Bge { offset, .. }
            | Inst::Bltu { offset, .. }
            | Inst::Bgeu { offset, .. } => Some(offset.as_i32()),
            Inst::Lui { .. }
            | Inst::Lb { .. }
            | Inst::Lbu { .. }
            | Inst::Lh { .. }
            | Inst::Lhu { .. }
            | Inst::Lw { .. }
            | Inst::Sb { .. }
            | Inst::Sh { .. }
            | Inst::Sw { .. }
            | Inst::Addi { .. }
            | Inst::Slti { .. }
            | Inst::Sltiu { .. }
            | Inst::Xori { .. }
            | Inst::Ori { .. }
            | Inst::Andi { .. }
            | Inst::Slli { .. }
            | Inst::Srli { .. }
            | Inst::Srai { .. }
            | Inst::Add { .. }
            | Inst::Sub { .. }
            | Inst::Sll { .. }
            | Inst::Slt { .. }
            | Inst::Sltu { .. }
            | Inst::Xor { .. }
            | Inst::Srl { .. }
            | Inst::Sra { .. }
            | Inst::Or { .. }
            | Inst::And { .. }
            | Inst::Fence { .. }
            | Inst::Mul { .. }
            | Inst::Mulh { .. }
            | Inst::Mulhsu { .. }
            | Inst::Mulhu { .. }
            | Inst::Div { .. }
            | Inst::Divu { .. }
            | Inst::Rem { .. }
            | Inst::Remu { .. }
            | Inst::LrW { .. }
            | Inst::ScW { .. }
            | Inst::AmoW { .. } => None,
            _ => {
                return Err(Gap::UnsupportedInstruction {
                    reason: format!(
                        "instruction {:?} has no semantic transfer proof",
                        instruction.instruction
                    ),
                });
            }
        };
        if let Some(displacement) = relative_target {
            let target = instruction
                .address
                .checked_add_signed(i64::from(displacement));
            if !instructions
                .iter()
                .any(|candidate| Some(candidate.address) == target)
            {
                return Err(Gap::InvalidControlFlow { offset });
            }
        }
    }
    if !instructions.last().is_some_and(|instruction| {
        matches!(instruction.instruction,
            Inst::Jal { dest, .. } | Inst::Jalr { dest, .. } if dest == Reg::ZERO)
    }) {
        return Err(Gap::OpenBodyBoundary);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
