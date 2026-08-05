//! Reference-codegen regression tests and shared generation helper.

use super::*;
use crate::Rv32CallArguments;
use crate::{DraftReferenceEvent, DraftReferenceFlow, DraftReferenceTerminator, FunctionAnalysis};

fn generate_from_trace(
    trace: &FunctionAnalysis,
    artifact: &str,
    artifact_sha256: &str,
    member: Option<&str>,
    companions: &[(String, String)],
) -> Result<GeneratedReference, String> {
    let program = ResolvedReferenceProgram::try_from(trace)?;
    generate(&program, artifact, artifact_sha256, member, companions)
}

mod calls;
mod generation;
mod memory;
mod polls;
mod value;
