//! Effect-contract-gated production candidate generation.

use super::super::*;

#[derive(Clone, Copy)]
enum OutputKind {
    PacLeaf,
    Transition,
    Plan,
}

impl OutputKind {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "pac-leaf" => Ok(Self::PacLeaf),
            "transition" => Ok(Self::Transition),
            "plan" => Ok(Self::Plan),
            _ => Err(format!(
                "unknown driver output kind {value:?}; expected pac-leaf, transition, or plan"
            )
            .into()),
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::PacLeaf => "pac-leaf",
            Self::Transition => "transition",
            Self::Plan => "plan",
        }
    }
}

pub(super) fn run(
    filtered: Vec<String>,
    svd: &MmioRegisterMap,
    target: &TargetSpec,
) -> Result<bool> {
    let mut artifact = None;
    let mut companions = Vec::new();
    let mut member = None;
    let mut symbol = None;
    let mut source = None;
    let mut dispositions = None;
    let mut pac_bindings = None;
    let mut output_kind = None;
    let mut output = None;
    let mut plan_output = None;
    let riscv_harness = harnesses::riscv(&target.harness)?;
    let mut entry_contract = harnesses::entry_contract(&target.harness, "none")?;
    let mut arguments = filtered.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--artifact" => {
                artifact = Some(PathBuf::from(take_value(&mut arguments, "--artifact")?));
            }
            "--companion" => {
                companions.push(PathBuf::from(take_value(&mut arguments, "--companion")?));
            }
            "--member" => member = Some(take_value(&mut arguments, "--member")?),
            "--symbol" => symbol = Some(take_value(&mut arguments, "--symbol")?),
            "--source" => source = Some(take_value(&mut arguments, "--source")?),
            "--dispositions" => {
                dispositions = Some(PathBuf::from(take_value(&mut arguments, "--dispositions")?));
            }
            "--pac-bindings" => {
                pac_bindings = Some(PathBuf::from(take_value(&mut arguments, "--pac-bindings")?));
            }
            "--kind" => {
                output_kind = Some(OutputKind::parse(&take_value(&mut arguments, "--kind")?)?);
            }
            "--output" => output = Some(PathBuf::from(take_value(&mut arguments, "--output")?)),
            "--plan-output" => {
                plan_output = Some(PathBuf::from(take_value(&mut arguments, "--plan-output")?));
            }
            "--entry-contract" => {
                entry_contract = harnesses::entry_contract(
                    &target.harness,
                    &take_value(&mut arguments, "--entry-contract")?,
                )?;
            }
            _ => return Err(format!("unknown driver generate option: {argument}").into()),
        }
    }

    let artifact = artifact.ok_or("missing --artifact")?;
    let symbol = symbol.ok_or("missing --symbol")?;
    let source = source.ok_or("missing --source")?;
    dispositions::validate_source_id(&source, 0)?;
    let dispositions = dispositions
        .or_else(|| target.dispositions.clone())
        .ok_or("driver generation requires --dispositions or target dispositions")?;
    let pac_bindings = pac_bindings
        .or_else(|| target.pac_bindings.clone())
        .ok_or("driver generation requires --pac-bindings or target pac-bindings")?;
    let output_kind = output_kind.ok_or("missing --kind")?;

    let manifest = dispositions::Manifest::load(&dispositions)?;
    let resolved_disposition = manifest.resolve(&source, &symbol);
    let entry = resolved_disposition.entry.ok_or_else(|| {
        format!("driver generation requires an explicit disposition for {source} {symbol}")
    })?;
    let policy = entry.effect_contract.as_ref().ok_or_else(|| {
        format!("driver generation requires an effect-contract for {source} {symbol}")
    })?;

    let selector = ArtifactSymbolSelector {
        artifact,
        member,
        symbol: symbol.clone(),
    };
    let trace = extract_reference(&selector, &companions, riscv_harness, entry_contract, svd)?;
    let resolved =
        ResolvedReferenceProgram::try_from(&trace).map_err(|error| -> Error { error.into() })?;
    let bindings = effect_contract::PacBindingIndex::load(&pac_bindings)?;
    let plan = effect_contract::DriverPlan::from_resolved(&resolved, policy, &bindings)?;
    let plan_source = plan.canonical();
    if let Some(path) = plan_output {
        fs::write(path, &plan_source)?;
    }
    let generated = match output_kind {
        OutputKind::PacLeaf => effect_contract::lower_pac_leaf(&plan, &bindings.crate_name)?.source,
        OutputKind::Transition => effect_contract::lower_transition_skeleton(&plan)?.source,
        OutputKind::Plan => plan_source,
    };
    if let Some(path) = output {
        fs::write(&path, generated)?;
        println!(
            "GENERATED-DRIVER\t{}\t{}\tkind={}\tsource={source}",
            symbol,
            path.display(),
            output_kind.label()
        );
    } else {
        print!("{generated}");
    }
    Ok(true)
}
