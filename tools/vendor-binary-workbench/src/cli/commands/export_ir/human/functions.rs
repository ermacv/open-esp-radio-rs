//! Per-function evidence and transitive-effect sections.

use super::super::*;

mod effects;
mod local;

pub(super) fn render(output: &mut String, report: &LinkedIrReport) {
    for function in &report.functions {
        local::render(output, function);
        effects::render(output, function);
    }
}

pub(super) fn memory_object_label(object: &LinkedMemoryObject) -> String {
    match object {
        LinkedMemoryObject::Argument { index } => format!("argument:{index}"),
        LinkedMemoryObject::Global { member, symbol } => format!(
            "global:{}::{symbol}",
            member.as_deref().unwrap_or("<linked>")
        ),
        LinkedMemoryObject::DereferencedGlobal {
            member,
            symbol,
            pointer_offset,
        } => format!(
            "dereferenced-global:{}::{symbol}{pointer_offset:+#x}",
            member.as_deref().unwrap_or("<linked>")
        ),
        LinkedMemoryObject::Absolute {
            address_space,
            address,
        } => format!("absolute:{address_space}:{address:#010x}"),
    }
}
