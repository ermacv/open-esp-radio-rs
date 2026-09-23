//! Executable coverage is accounted independently of named MMIO users.
use super::*;

pub(super) fn observations(
    source: &str,
    capture: &artifact::CapturedArtifact<'_>,
    selected: &[artifact::ArtifactSymbolDefinition],
) -> Vec<serde_json::Value> {
    let inventory = match capture.inventory() {
        Ok(inventory) => inventory,
        Err(error) => {
            return vec![
                serde_json::json!({"kind":"code-coverage","source":source,"reason":error.to_string(),"complete":false}),
            ];
        }
    };
    let mut output = Vec::new();
    for object in &inventory.objects {
        for section in &object.code_sections {
            let skipped = object.symbols.iter().filter(|symbol| symbol.size != 0 && symbol.section.as_deref() == Some(&section.name) && !selected.iter().any(|selected| selected.member == object.member && selected.name == symbol.name && selected.address == symbol.address)).map(|symbol| serde_json::json!({"symbol":symbol.name,"address":symbol.address,"size":symbol.size})).collect::<Vec<_>>();
            let uncovered = section
                .uncovered_ranges
                .iter()
                .map(|range| (range.start_offset, range.end_offset))
                .collect::<Vec<_>>();
            let candidates = section.function_candidates.iter().map(|candidate| serde_json::json!({"entry_offset":candidate.entry_offset,"end_limit_offset":candidate.end_limit_offset,"symbols":candidate.symbol_names})).collect::<Vec<_>>();
            let blockers = section.recovery_blockers.iter().map(|blocker| serde_json::json!({"symbol":blocker.symbol,"message":blocker.message})).collect::<Vec<_>>();
            output.push(serde_json::json!({"kind":"code-coverage","source":source,"member":object.member,"section":section.name,"address":section.address,"size":section.size,"uncovered_by_sized_symbols":uncovered,"boundary_candidates":candidates,"recovery_blockers":blockers,"unselected_symbols":skipped,"named_zero_sized_symbols":section.named_zero_sized_symbols,"complete":false,"reason":"symbol boundaries and root selection do not prove successful decoding or complete path exploration; reviewed recovered boundaries and per-function diagnostics are reported separately"}));
        }
    }
    for member in inventory
        .members
        .iter()
        .filter(|member| member.reason.is_some())
    {
        output.push(serde_json::json!({"kind":"code-coverage","source":source,"member":member,"complete":false,"reason":"artifact member was not fully inventoried"}));
    }
    output
}
