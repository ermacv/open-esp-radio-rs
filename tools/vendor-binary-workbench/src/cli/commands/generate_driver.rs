//! Effect-contract-gated production candidate generation.

use super::super::*;

use serde::Serialize;

#[derive(Serialize)]
struct GeneratedDriverReport {
    schema_version: u32,
    command: &'static str,
    vendor_source: String,
    vendor_symbol: String,
    kind: &'static str,
    output: Option<String>,
    plan_output: Option<String>,
    source: String,
}

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
            _ => Err(crate::Error::invalid(format!(
                "unknown driver output kind {value:?}; expected pac-leaf, transition, or plan"
            ))),
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
    arguments: DriverGenerateArgs,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    let harness = target.require_available_harness()?;
    let riscv_harness = harnesses::riscv(harness)?;
    let entry_contract = harnesses::entry_contract(harness, &arguments.entry_contract)?;
    let artifact = arguments
        .artifact
        .ok_or("missing --artifact")
        .map_err(crate::Error::invalid)?;
    let symbol = arguments
        .symbol
        .ok_or("missing --symbol")
        .map_err(crate::Error::invalid)?;
    let source = arguments
        .source
        .ok_or("missing --source")
        .map_err(crate::Error::invalid)?;
    dispositions::validate_source_id(&source, 0)?;
    let dispositions = arguments
        .dispositions
        .or_else(|| target.dispositions.clone())
        .ok_or("driver generation requires --dispositions or target dispositions")
        .map_err(crate::Error::invalid)?;
    let pac_bindings = arguments
        .pac_bindings
        .or_else(|| target.pac_bindings.clone())
        .ok_or("driver generation requires --pac-bindings or target pac-bindings")
        .map_err(crate::Error::invalid)?;
    let output_kind = OutputKind::parse(
        arguments
            .kind
            .as_deref()
            .ok_or("missing --kind")
            .map_err(crate::Error::invalid)?,
    )?;

    let manifest = dispositions::Manifest::load(&dispositions)?;
    let resolved_disposition = manifest.resolve(&source, &symbol);
    let entry = resolved_disposition
        .entry
        .ok_or_else(|| {
            format!("driver generation requires an explicit disposition for {source} {symbol}")
        })
        .map_err(crate::Error::invalid)?;
    let policy = entry
        .effect_contract
        .as_ref()
        .ok_or_else(|| {
            format!("driver generation requires an effect-contract for {source} {symbol}")
        })
        .map_err(crate::Error::invalid)?;

    let selector = ArtifactSymbolSelector {
        artifact,
        member: arguments.member,
        symbol: symbol.clone(),
    };
    let trace = extract_reference(
        &selector,
        &arguments.companion,
        riscv_harness,
        entry_contract,
        svd,
    )?;
    let resolved = ResolvedReferenceProgram::try_from(&trace)
        .map_err(|error| -> Error { crate::Error::invalid(error) })?;
    let bindings = effect_contract::PacBindingIndex::load(&pac_bindings)?;
    let plan = effect_contract::DriverPlan::from_resolved(&resolved, policy, &bindings)?;
    let plan_source = plan.canonical();
    if let Some(path) = arguments.plan_output.as_deref() {
        fs::write(path, &plan_source)?;
    }
    let generated = match output_kind {
        OutputKind::PacLeaf => effect_contract::lower_pac_leaf(&plan, &bindings.crate_name)?.source,
        OutputKind::Transition => effect_contract::lower_transition_skeleton(&plan)?.source,
        OutputKind::Plan => plan_source,
    };
    if let Some(path) = arguments.output.as_deref() {
        fs::write(path, &generated)?;
    }
    let report = GeneratedDriverReport {
        schema_version: 1,
        command: "driver generate",
        vendor_source: source,
        vendor_symbol: symbol,
        kind: output_kind.label(),
        output: arguments.output.map(|path| path.display().to_string()),
        plan_output: arguments.plan_output.map(|path| path.display().to_string()),
        source: generated,
    };
    if !crate::cli::output::structured(&report) {
        if let Some(output) = &report.output {
            outputln!(
                "GENERATED-DRIVER\t{}\t{}\tkind={}\tsource={}",
                report.vendor_symbol,
                output,
                report.kind,
                report.vendor_source
            );
        } else {
            crate::cli::output::text(&report.source);
        }
    }
    Ok(true)
}
