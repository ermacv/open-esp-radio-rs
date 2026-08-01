//! Safe Rust reference generation command.

use super::super::*;

pub(super) fn run(filtered: Vec<String>, svd: &MmioRegisterMap) -> Result<bool> {
    let mut artifact = None;
    let mut companions = Vec::new();
    let mut member = None;
    let mut symbol = None;
    let mut output = None;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--artifact" => {
                artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
            }
            "--companion" => {
                companions.push(PathBuf::from(take_value(&mut arguments, "--companion")?));
            }
            "--member" => {
                member = Some(take_value(&mut arguments, "--member")?);
            }
            "--symbol" => {
                symbol = Some(take_value(&mut arguments, "--symbol")?);
            }
            "--output" => {
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            _ => {
                return Err(format!("unknown generate-reference option: {argument}").into());
            }
        }
    }
    let artifact = artifact.ok_or("missing --artifact")?;
    let symbol = symbol.ok_or("missing --symbol")?;
    let input = ArtifactSymbolSelector {
        artifact: artifact.clone(),
        member: member.clone(),
        symbol,
    };
    let trace = extract_reference(&input, &companions, svd)?;
    let resolved =
        ResolvedReferenceProgram::try_from(&trace).map_err(|error| -> Error { error.into() })?;
    let digest = artifact_sha256(&artifact)?;
    let companion_provenance = companions
        .iter()
        .map(|companion| Ok((companion.display().to_string(), artifact_sha256(companion)?)))
        .collect::<Result<Vec<_>>>()?;
    let generated = codegen::generate(
        &resolved,
        &artifact.display().to_string(),
        &digest,
        member.as_deref(),
        &companion_provenance,
    )
    .map_err(|error| -> Error { error.into() })?;
    if let Some(output) = output {
        fs::write(&output, generated.source)?;
        println!(
            "GENERATED\t{}\t{}\texit-a0={}",
            trace.symbol,
            output.display(),
            if generated.exit_a0_modeled {
                "modeled"
            } else {
                "unresolved"
            }
        );
    } else {
        print!("{}", generated.source);
    }
    Ok(true)
}
