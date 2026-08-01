use std::{fmt::Write, fs, path::Path};

use crate::{
    Result,
    model::{HilProof, Ledger, VendorProof},
};

struct Summary {
    implementation_complete: usize,
    host_covered: usize,
    vendor_qualified: usize,
    hil_qualified: usize,
    async_terminal: usize,
    proof_ready: usize,
    ready: usize,
}

fn summarize(ledger: &Ledger) -> Summary {
    Summary {
        implementation_complete: ledger
            .capabilities
            .values()
            .filter(|capability| capability.implementation.is_terminal())
            .count(),
        host_covered: ledger
            .capabilities
            .values()
            .filter(|capability| capability.host.is_terminal())
            .count(),
        vendor_qualified: ledger
            .capabilities
            .values()
            .filter(|capability| capability.vendor == VendorProof::Qualified)
            .count(),
        hil_qualified: ledger
            .capabilities
            .values()
            .filter(|capability| capability.hil == HilProof::Qualified)
            .count(),
        async_terminal: ledger
            .capabilities
            .values()
            .filter(|capability| capability.async_proof.is_terminal())
            .count(),
        proof_ready: ledger
            .capabilities
            .values()
            .filter(|capability| capability.proof_ready())
            .count(),
        ready: ledger
            .capabilities
            .keys()
            .filter(|id| ledger.is_ready(id))
            .count(),
    }
}

pub(crate) fn print(ledger: &Ledger) {
    for capability in ledger.capabilities.values() {
        println!(
            "CAPABILITY\t{}\timplementation={}\thost={}\tvendor={}\thil={}\tasync={}\tproof-ready={}\tready={}",
            capability.id,
            capability.implementation.label(),
            capability.host.label(),
            capability.vendor.label(),
            capability.hil.label(),
            capability.async_proof.label(),
            capability.proof_ready(),
            ledger.is_ready(&capability.id),
        );
        for gap in &capability.gaps {
            println!(
                "GAP\t{}\taxis={}\tid={}",
                capability.id,
                gap.axis.label(),
                gap.id
            );
        }
    }
    let summary = summarize(ledger);
    println!(
        "SUMMARY\ttarget={}\tcapabilities={}\timplementation-complete={}\thost-covered={}\tvendor-qualified={}\thil-qualified={}\tasync-terminal={}\tproof-ready={}\tready={}",
        ledger.target,
        ledger.capabilities.len(),
        summary.implementation_complete,
        summary.host_covered,
        summary.vendor_qualified,
        summary.hil_qualified,
        summary.async_terminal,
        summary.proof_ready,
        summary.ready,
    );
}

fn json_string(output: &mut String, value: &str) {
    output.push('"');
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            character if character.is_control() => {
                write!(output, "\\u{:04x}", character as u32)
                    .expect("writing to String cannot fail");
            }
            character => output.push(character),
        }
    }
    output.push('"');
}

pub(crate) fn write_json(ledger: &Ledger, path: &Path) -> Result<()> {
    let mut output = String::new();
    output.push_str("{\n  \"schema_version\": 1,\n  \"target\": ");
    json_string(&mut output, &ledger.target);
    output.push_str(",\n  \"capabilities\": [\n");
    for (index, capability) in ledger.capabilities.values().enumerate() {
        output.push_str("    {\"id\": ");
        json_string(&mut output, &capability.id);
        output.push_str(", \"title\": ");
        json_string(&mut output, &capability.title);
        output.push_str(", \"scope\": ");
        json_string(&mut output, &capability.scope);
        write!(
            output,
            ", \"implementation\": \"{}\", \"host\": \"{}\", \"vendor\": \"{}\", \"hil\": \"{}\", \"async\": \"{}\", \"proof_ready\": {}, \"ready\": {}, \"dependencies\": [",
            capability.implementation.label(),
            capability.host.label(),
            capability.vendor.label(),
            capability.hil.label(),
            capability.async_proof.label(),
            capability.proof_ready(),
            ledger.is_ready(&capability.id),
        )
        .expect("writing to String cannot fail");
        for (dependency_index, dependency) in capability.dependencies.iter().enumerate() {
            if dependency_index != 0 {
                output.push_str(", ");
            }
            json_string(&mut output, dependency);
        }
        output.push_str("], \"gaps\": [");
        for (gap_index, gap) in capability.gaps.iter().enumerate() {
            if gap_index != 0 {
                output.push_str(", ");
            }
            output.push_str("{\"axis\": ");
            json_string(&mut output, gap.axis.label());
            output.push_str(", \"id\": ");
            json_string(&mut output, &gap.id);
            output.push('}');
        }
        output.push_str("]}");
        output.push_str(if index + 1 == ledger.capabilities.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    let summary = summarize(ledger);
    writeln!(
        output,
        "  ],\n  \"summary\": {{\"capabilities\": {}, \"implementation_complete\": {}, \"host_covered\": {}, \"vendor_qualified\": {}, \"hil_qualified\": {}, \"async_terminal\": {}, \"proof_ready\": {}, \"ready\": {}}}\n}}",
        ledger.capabilities.len(),
        summary.implementation_complete,
        summary.host_covered,
        summary.vendor_qualified,
        summary.hil_qualified,
        summary.async_terminal,
        summary.proof_ready,
        summary.ready,
    )
    .expect("writing to String cannot fail");
    fs::write(path, output)?;
    Ok(())
}
