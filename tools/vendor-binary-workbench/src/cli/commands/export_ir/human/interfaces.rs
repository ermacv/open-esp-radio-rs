//! Semantic boundary and trampoline-interface sections.

use std::fmt::Write as _;

use super::super::*;

pub(super) fn render(mut output: &mut String, report: &LinkedIrReport) {
    for boundary in &report.semantic_boundaries {
        let _ = writeln!(
            &mut output,
            "SEMANTIC\t{}\tcall-shapes={}\tfunctions={}\ttargets={}\treplacements={}",
            boundary.operation,
            boundary.call_shapes,
            boundary.functions.join(","),
            boundary.targets.join(","),
            boundary.replacement_hints.join(" | "),
        );
    }
    for slot in &report.trampoline_slots {
        let trampoline = &slot.trampoline;
        let _ = writeln!(
            &mut output,
            "TRAMPOLINE-SLOT\t{}+{:#x}\tfunction={}\tid={}\tversion={}\tpointer={}\tbacking={}\tmagic={:#010x}@{:#x}\ttable-size={:#x}\targs={}\treturn-model={}\treturn-type={}\toperation={}\treplacement={}\tcall-shapes={}\tfunctions={}",
            trampoline.table,
            trampoline.slot,
            trampoline.c_name,
            trampoline.function_id,
            trampoline.version,
            trampoline.pointer_symbol,
            trampoline.backing_symbol,
            trampoline.magic,
            trampoline.magic_offset,
            trampoline.table_size,
            trampoline.argument_count,
            trampoline.return_model,
            trampoline.return_type,
            trampoline.operation,
            trampoline.replacement_hint.as_deref().unwrap_or("-"),
            slot.call_shapes,
            slot.functions.join(","),
        );
        for argument in &slot.arguments {
            let _ = writeln!(
                &mut output,
                "TRAMPOLINE-ARG\t{}+{:#x}\tposition={}\tname={}\ttype={}\tdirection={}",
                trampoline.table,
                trampoline.slot,
                argument.position,
                argument.name,
                argument.c_type,
                argument.direction,
            );
        }
    }
}
