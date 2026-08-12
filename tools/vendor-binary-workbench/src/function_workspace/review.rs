//! Human-oriented function and context review report.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use super::{
    FunctionFact, FunctionInterfaceLink, FunctionMemoryObjectFact, FunctionReviewStatus,
    FunctionWorkspace, ReviewedFunction, ReviewedLogicalType, ReviewedMemoryObject,
    ReviewedTypeField,
};
use crate::Result;

const DECODE_BLOCKER_PREVIEW_LIMIT: usize = 16;

pub(crate) fn render_function_review(
    workspace: &FunctionWorkspace,
    interface_links: Option<&[FunctionInterfaceLink]>,
) -> Result<String> {
    let mut output = String::new();
    let summary = workspace.summary();
    output.push_str("# Function review report\n\n");
    output.push_str(
        "Generated from the reviewed function pack and its selected linked-IR profiles.\n",
    );
    output.push_str(
        "This report is derived navigation material. Edit the function pack, not this file. Function names, roles, summaries and context names are reviewed claims; pseudo-code, access counts and inferred layouts remain generated evidence and do not claim source-level equivalence.\n\n",
    );
    output.push_str("## Inputs\n\n");
    for input in &workspace.facts.inputs {
        writeln!(
            output,
            "- `{}` / `{}` / `{}`",
            markdown_code(&input.profile),
            markdown_code(&input.source),
            input.sha256
        )
        .expect("writing to String cannot fail");
    }
    output.push_str("\n## Coverage\n\n");
    writeln!(
        output,
        "- Observed root functions: {}",
        summary.observed_functions
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Reviewed root functions: {}",
        summary.reviewed_functions
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Ignored root functions: {}",
        summary.ignored_functions
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Unreviewed root functions: {}",
        summary.unreviewed_functions
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Contexts: {} reviewed, {} ignored, {} unreviewed",
        summary.reviewed_contexts, summary.ignored_contexts, summary.unreviewed_contexts
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Context fields: {} reviewed, {} ignored, {} unreviewed",
        summary.reviewed_fields, summary.ignored_fields, summary.unreviewed_fields
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Reviewed functions with explicitly accepted incomplete evidence: {}",
        summary.accepted_incomplete
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Logical types: {} with {} bindings and {} classified fields",
        summary.logical_types, summary.type_bindings, summary.type_fields
    )
    .expect("writing to String cannot fail");

    if !workspace.pack.types.is_empty() {
        output.push_str("\n## Reviewed logical types\n\n");
        output.push_str(
            "Types are explicit reviewer-owned unification claims over generated memory-object evidence. Matching offsets alone never create a type.\n",
        );
        for logical_type in &workspace.pack.types {
            writeln!(
                output,
                "\n### `{}` — `{}`\n",
                markdown_code(&logical_type.name),
                markdown_code(&logical_type.id)
            )
            .expect("writing to String cannot fail");
            if let Some(description) = &logical_type.description {
                writeln!(output, "{}\n", markdown_text(description))
                    .expect("writing to String cannot fail");
            }
            output.push_str("Bindings:\n\n");
            for binding in &logical_type.bindings {
                writeln!(
                    output,
                    "- `{}` = `{}` in `{}` / `{}`",
                    markdown_code(&binding.name),
                    markdown_code(&reviewed_object_label(&binding.object)),
                    markdown_code(&binding.profile),
                    markdown_code(&binding.source)
                )
                .expect("writing to String cannot fail");
            }
            output.push_str("\n| Offset | Width | Access | Vendor functions | Status | Name | Display type | Description |\n");
            output.push_str("| ---: | ---: | --- | --- | --- | --- | --- | --- |\n");
            for field in &logical_type.fields {
                let usage = logical_type_field_usage(workspace, logical_type, field);
                writeln!(
                    output,
                    "| `{:+#x}` | {} | {} | {} | {} | {} | {} | {} |",
                    field.offset,
                    field.width,
                    usage.access_label(),
                    usage.function_preview(),
                    review_status(field.status),
                    field
                        .name
                        .as_deref()
                        .map(markdown_code)
                        .unwrap_or_else(|| "-".to_owned()),
                    field
                        .display_type
                        .as_deref()
                        .map(markdown_code)
                        .unwrap_or_else(|| "-".to_owned()),
                    field
                        .description
                        .as_deref()
                        .map(markdown_text)
                        .unwrap_or_else(|| "-".to_owned()),
                )
                .expect("writing to String cannot fail");
            }
        }
    }

    let reviewed = workspace
        .pack
        .functions
        .iter()
        .map(|function| {
            (
                (
                    function.profile.as_str(),
                    function.source.as_str(),
                    function.identity.as_str(),
                ),
                function,
            )
        })
        .collect::<BTreeMap<_, _>>();
    output.push_str("\n## Reviewed function reading views\n\n");
    output.push_str(
        "Detailed pseudo-code and joined interface evidence are emitted only for explicit `reviewed` overlay entries. Complete generated pseudo-code remains in each linked-IR profile's `.pseudo.rs` report and in the TUI.\n",
    );
    for fact in &workspace.facts.functions {
        let function = reviewed_function(fact, &reviewed);
        if function.is_some_and(|function| function.status == FunctionReviewStatus::Reviewed) {
            write_fact(&mut output, fact, &reviewed, interface_links)?;
        }
    }

    let ignored = workspace.facts.functions.iter().filter(|fact| {
        reviewed_function(fact, &reviewed)
            .is_some_and(|function| function.status == FunctionReviewStatus::Ignored)
    });
    write_function_inventory(&mut output, "Ignored function decisions", ignored);

    let unreviewed_roots = workspace
        .facts
        .root_functions()
        .filter(|fact| reviewed_function(fact, &reviewed).is_none());
    write_function_inventory(
        &mut output,
        "Unreviewed root function inventory",
        unreviewed_roots,
    );

    let unreviewed_reachable = workspace
        .facts
        .functions
        .iter()
        .filter(|fact| !fact.is_root() && reviewed_function(fact, &reviewed).is_none());
    write_function_inventory(
        &mut output,
        "Unreviewed reachable function inventory",
        unreviewed_reachable,
    );
    Ok(output)
}

#[derive(Default)]
struct LogicalTypeFieldUsage {
    read: bool,
    written: bool,
    functions: BTreeSet<String>,
}

impl LogicalTypeFieldUsage {
    fn access_label(&self) -> &'static str {
        match (self.read, self.written) {
            (true, true) => "R/W",
            (true, false) => "R",
            (false, true) => "W",
            (false, false) => "-",
        }
    }

    fn function_preview(&self) -> String {
        const LIMIT: usize = 6;
        let mut names = self
            .functions
            .iter()
            .take(LIMIT)
            .map(|name| format!("`{}`", markdown_code(name)))
            .collect::<Vec<_>>();
        if self.functions.len() > LIMIT {
            names.push(format!("+{} more", self.functions.len() - LIMIT));
        }
        if names.is_empty() {
            "-".to_owned()
        } else {
            names.join("<br>")
        }
    }
}

fn logical_type_field_usage(
    workspace: &FunctionWorkspace,
    logical_type: &ReviewedLogicalType,
    reviewed_field: &ReviewedTypeField,
) -> LogicalTypeFieldUsage {
    let mut usage = LogicalTypeFieldUsage::default();
    for binding in &logical_type.bindings {
        for function in workspace.facts.functions.iter().filter(|function| {
            function.profile == binding.profile && function.source == binding.source
        }) {
            for field in function.memory_fields.iter().filter(|field| {
                field.offset == reviewed_field.offset
                    && field.width == reviewed_field.width
                    && reviewed_object_matches(&binding.object, &field.object, function)
            }) {
                usage.read |= field.reads != 0;
                usage.written |= field.writes != 0;
                if field.origins.is_empty() {
                    usage.functions.insert(function.identity.clone());
                } else {
                    usage.functions.extend(field.origins.iter().cloned());
                }
            }
        }
    }
    usage
}

fn reviewed_object_matches(
    reviewed: &ReviewedMemoryObject,
    observed: &FunctionMemoryObjectFact,
    function: &FunctionFact,
) -> bool {
    match (reviewed, observed) {
        (
            ReviewedMemoryObject::Argument {
                function: expected,
                index: left,
            },
            FunctionMemoryObjectFact::Argument { index: right },
        ) => (function.identity == *expected || function.symbol == *expected) && left == right,
        (
            ReviewedMemoryObject::Global {
                member: left_member,
                symbol: left_symbol,
            },
            FunctionMemoryObjectFact::Global {
                member: right_member,
                symbol: right_symbol,
            },
        ) => left_member == right_member && left_symbol == right_symbol,
        (
            ReviewedMemoryObject::Dereferenced {
                pointer: left,
                pointer_offset: left_offset,
            },
            FunctionMemoryObjectFact::Dereferenced {
                pointer: right,
                pointer_offset: right_offset,
            },
        ) => left_offset == right_offset && reviewed_object_matches(left, right, function),
        (
            ReviewedMemoryObject::Absolute {
                address_space: left_space,
                address: left_address,
            },
            FunctionMemoryObjectFact::Absolute {
                address_space: right_space,
                address: right_address,
            },
        ) => left_space == right_space && left_address == right_address,
        _ => false,
    }
}

fn reviewed_function<'a>(
    fact: &FunctionFact,
    reviewed: &'a BTreeMap<(&str, &str, &str), &ReviewedFunction>,
) -> Option<&'a ReviewedFunction> {
    reviewed
        .get(&(
            fact.profile.as_str(),
            fact.source.as_str(),
            fact.identity.as_str(),
        ))
        .copied()
}

fn write_function_inventory<'a>(
    output: &mut String,
    title: &str,
    facts: impl Iterator<Item = &'a FunctionFact>,
) {
    let facts = facts.collect::<Vec<_>>();
    if facts.is_empty() {
        return;
    }
    writeln!(output, "\n## {title}\n").expect("writing to String cannot fail");
    output.push_str(
        "This is a navigation index, not a source reconstruction. Use the profile pseudo-code, linked IR, JSON output or TUI for complete per-function evidence.\n\n",
    );
    output.push_str(
        "| Profile | Source | Function | Evidence closure | Calls | MMIO | Context fields | Memory fields | Semantics | Blockers |\n",
    );
    output.push_str("| --- | --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n");
    for fact in facts {
        let blockers = fact.decode_blockers.len() + fact.context_projection_blockers.len();
        writeln!(
            output,
            "| `{}` | `{}` | `{}` | `{}/{}/{}` | {} | {} | {} | {} | {} | {} |",
            markdown_code(&fact.profile),
            markdown_code(&fact.source),
            markdown_code(&fact.symbol),
            yes_no(fact.direct_complete),
            yes_no(fact.call_graph_closed),
            yes_no(fact.context_projection_complete),
            fact.calls.len(),
            fact.mmio_addresses.len(),
            fact.context_fields.len(),
            fact.memory_fields.len(),
            fact.semantic_operations.len(),
            blockers,
        )
        .expect("writing to String cannot fail");
    }
}

fn reviewed_object_label(object: &ReviewedMemoryObject) -> String {
    match object {
        ReviewedMemoryObject::Argument { function, index } => {
            format!("{function}::arg{index}")
        }
        ReviewedMemoryObject::Global { member, symbol } => {
            format!("{}::{symbol}", member.as_deref().unwrap_or("<linked>"))
        }
        ReviewedMemoryObject::Dereferenced {
            pointer,
            pointer_offset,
        } => format!("*({}{pointer_offset:+#x})", reviewed_object_label(pointer)),
        ReviewedMemoryObject::Absolute {
            address_space,
            address,
        } => format!("absolute<{address_space}>({address:#010x})"),
    }
}

fn review_status(status: FunctionReviewStatus) -> &'static str {
    match status {
        FunctionReviewStatus::Reviewed => "reviewed",
        FunctionReviewStatus::Ignored => "ignored",
    }
}

fn write_fact(
    output: &mut String,
    fact: &FunctionFact,
    reviewed: &BTreeMap<(&str, &str, &str), &ReviewedFunction>,
    interface_links: Option<&[FunctionInterfaceLink]>,
) -> Result<()> {
    let function = reviewed.get(&(
        fact.profile.as_str(),
        fact.source.as_str(),
        fact.identity.as_str(),
    ));
    let links = interface_links
        .unwrap_or_default()
        .iter()
        .filter(|link| {
            link.profile == fact.profile
                && link.source == fact.source
                && link.identity == fact.identity
        })
        .collect::<Vec<_>>();
    write_function(output, fact, function.copied(), &links)
}

fn write_function(
    output: &mut String,
    fact: &FunctionFact,
    reviewed: Option<&ReviewedFunction>,
    interface_links: &[&FunctionInterfaceLink],
) -> Result<()> {
    let state = reviewed.map_or("unreviewed", |function| match function.status {
        FunctionReviewStatus::Reviewed => "reviewed",
        FunctionReviewStatus::Ignored => "ignored",
    });
    let title = reviewed
        .and_then(|function| function.name.as_deref())
        .unwrap_or(&fact.symbol);
    writeln!(output, "\n### `{}` — {}\n", markdown_code(title), state)
        .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Profile/source: `{}` / `{}`",
        markdown_code(&fact.profile),
        markdown_code(&fact.source)
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "- Generated identity: `{}`",
        markdown_code(&fact.identity)
    )
    .expect("writing to String cannot fail");
    writeln!(output, "- Generated selection: `{}`", fact.selection)
        .expect("writing to String cannot fail");
    if let Some(function) = reviewed {
        if let Some(role) = &function.role {
            writeln!(output, "- Reviewed role: `{}`", markdown_code(role))
                .expect("writing to String cannot fail");
        }
        if let Some(summary) = &function.summary {
            writeln!(output, "- Reviewed summary: {}", markdown_text(summary))
                .expect("writing to String cannot fail");
        }
    }
    writeln!(
        output,
        "- Evidence closure: direct={}, calls={}, contexts={}{}",
        yes_no(fact.direct_complete),
        yes_no(fact.call_graph_closed),
        yes_no(fact.context_projection_complete),
        if reviewed.is_some_and(|function| function.accept_incomplete) {
            " (incompleteness explicitly accepted)"
        } else {
            ""
        }
    )
    .expect("writing to String cannot fail");
    if !fact.context_projection_blockers.is_empty() {
        writeln!(
            output,
            "- Context blockers: {}",
            fact.context_projection_blockers
                .iter()
                .map(|blocker| format!("`{}`", markdown_code(blocker)))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .expect("writing to String cannot fail");
    }
    if !fact.decode_blockers.is_empty() {
        writeln!(
            output,
            "- Decode blockers: {} total{}",
            fact.decode_blockers.len(),
            if fact.decode_blockers.len() > DECODE_BLOCKER_PREVIEW_LIMIT {
                "; first 16 shown"
            } else {
                ""
            }
        )
        .expect("writing to String cannot fail");
        for blocker in fact
            .decode_blockers
            .iter()
            .take(DECODE_BLOCKER_PREVIEW_LIMIT)
        {
            writeln!(
                output,
                "  - `{}` at `{:#x}`, width {}, raw `{:#010x}`, flow `{}`",
                markdown_code(&blocker.class),
                blocker.address,
                blocker.width,
                blocker.raw,
                if blocker.linear_control_flow {
                    "linear"
                } else {
                    "blocked"
                }
            )
            .expect("writing to String cannot fail");
        }
        if fact.decode_blockers.len() > DECODE_BLOCKER_PREVIEW_LIMIT {
            writeln!(
                output,
                "  - {} more; inspect linked IR or the TUI for complete per-PC evidence",
                fact.decode_blockers.len() - DECODE_BLOCKER_PREVIEW_LIMIT
            )
            .expect("writing to String cannot fail");
        }
    }
    writeln!(
        output,
        "- Recovered effects: {} semantic operation(s), {} trampoline call(s), {} event dispatch(es)",
        fact.semantic_operations.len(),
        fact.trampoline_calls,
        fact.event_dispatches.len()
    )
    .expect("writing to String cannot fail");
    if !fact.semantic_operations.is_empty() {
        writeln!(
            output,
            "- Semantic links: {}",
            fact.semantic_operations
                .iter()
                .map(|operation| format!("`{}`", markdown_code(operation)))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .expect("writing to String cannot fail");
    }
    write_interface_links(output, interface_links);
    write_contexts(output, fact, reviewed)?;
    write_memory_objects(output, fact);
    output.push_str("\n#### Generated pseudo-code\n\n```text\n");
    output.push_str(&fence_text(&fact.pseudo));
    if !fact.pseudo.ends_with('\n') {
        output.push('\n');
    }
    output.push_str("```\n");
    Ok(())
}

fn write_memory_objects(output: &mut String, fact: &FunctionFact) {
    if fact.memory_fields.is_empty() {
        return;
    }
    output.push_str("\n#### Generated memory-object fields\n\n");
    output.push_str("| Object | Offset | Width | Reads | Writes | Write mask |\n");
    output.push_str("| --- | ---: | ---: | ---: | ---: | ---: |\n");
    for field in &fact.memory_fields {
        let object = function_memory_object_label(&field.object);
        writeln!(
            output,
            "| `{}` | `{:+#x}` | {} | {} | {} | `{:#010x}` |",
            markdown_code(&object),
            field.offset,
            field.width,
            field.reads,
            field.writes,
            field.write_mask
        )
        .expect("writing to String cannot fail");
    }
}

fn function_memory_object_label(object: &super::FunctionMemoryObjectFact) -> String {
    match object {
        super::FunctionMemoryObjectFact::Argument { index } => format!("argument:{index}"),
        super::FunctionMemoryObjectFact::Global { member, symbol } => format!(
            "global:{}::{symbol}",
            member.as_deref().unwrap_or("<linked>")
        ),
        super::FunctionMemoryObjectFact::Dereferenced {
            pointer,
            pointer_offset,
        } => format!(
            "*({}{pointer_offset:+#x})",
            function_memory_object_label(pointer)
        ),
        super::FunctionMemoryObjectFact::Absolute {
            address_space,
            address,
        } => format!("absolute:{address_space}:{address:#010x}"),
        super::FunctionMemoryObjectFact::Indexed {
            object,
            argument,
            stride,
        } => format!(
            "{}[arg{argument} * {stride:#x}]",
            function_memory_object_label(object)
        ),
        super::FunctionMemoryObjectFact::ZeroedAllocation { call_token } => {
            format!("zeroed-allocation:{call_token}")
        }
    }
}

fn write_interface_links(output: &mut String, links: &[&FunctionInterfaceLink]) {
    if links.is_empty() {
        return;
    }
    output.push_str("\n#### Validated interface call sites\n\n");
    output.push_str(
        "Each row joins a reviewed interface slot to a concrete static call instruction and the argument expressions recovered by generic provenance analysis. When schema-v50 linked IR contains exactly the same caller and site, its factorized CFG guard paths are attached as separate evidence. The reviewed semantic is a catalog claim attached to that slot. This evidence does not establish runtime order, branch feasibility, callee side effects, return values or scheduler/storage behavior.\n\n",
    );
    output.push_str("| Contract/version | Slot | Static site | Caller/kind | Recovered arguments | Linked-IR CFG evidence | ABI | Semantic | Execution model |\n");
    output.push_str("| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for link in links {
        let mut abi_arguments = link.arguments.join(", ");
        if link.variadic {
            if !abi_arguments.is_empty() {
                abi_arguments.push_str(", ");
            }
            abi_arguments.push_str("...");
        }
        for call in &link.calls {
            let recovered_arguments = call
                .arguments
                .iter()
                .map(|(index, kind, expression)| {
                    format!(
                        "`a{}={}` ({})",
                        index,
                        markdown_code(expression),
                        markdown_text(kind)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            let location = call.member.as_ref().map_or_else(
                || format!("artifact {} / `{:#010x}`", call.artifact, call.site),
                |member| {
                    format!(
                        "artifact {} / `{}` / `{:#010x}`",
                        call.artifact,
                        markdown_code(member),
                        call.site
                    )
                },
            );
            writeln!(
                output,
                "| `{}` / `{}` | `{}` `{:+#x}/{}` `{}` | {} | `{}` @ `{:#010x}` ({}, jalr {:+#x}) | {} | {} | `({}) -> {}` | {} | {} |",
                markdown_code(&link.contract),
                markdown_code(&link.layout_version),
                markdown_code(&link.slot),
                link.offset,
                link.width,
                markdown_code(&link.name),
                location,
                markdown_code(&call.caller),
                call.function_address,
                markdown_text(
                    &call.slot_selector.as_ref().map_or_else(
                        || call.kind.clone(),
                        |selector| {
                            let domain = call.slot_index_domain.as_ref().map_or_else(
                                || "unproven-domain".to_owned(),
                                |(argument, min, max, evidence)| {
                                    format!("arg{argument}:{min}..={max} evidence={evidence}")
                                },
                            );
                            format!(
                                "{} indexed({selector}) index={} {domain}",
                                call.kind,
                                call.slot_index
                                    .map_or_else(|| "?".to_owned(), |index| index.to_string())
                            )
                        },
                    ),
                ),
                call.jalr_offset,
                if recovered_arguments.is_empty() {
                    "-"
                } else {
                    &recovered_arguments
                },
                linked_ir_evidence(call),
                markdown_code(&abi_arguments),
                markdown_code(&link.return_type),
                optional_code(link.semantic.as_deref()),
                optional_code(link.execution_model.as_deref()),
            )
            .expect("writing to String cannot fail");
        }
    }
}

fn linked_ir_evidence(call: &super::FunctionInterfaceCall) -> String {
    let Some(linked_ir) = &call.linked_ir else {
        return match call.linked_ir_matches {
            0 => "not present at this site".to_owned(),
            matches => format!("ambiguous: {matches} records at this site"),
        };
    };
    let guards = linked_ir.guard_paths.as_ref().map_or_else(
        || "guards unavailable".to_owned(),
        |paths| {
            paths
                .iter()
                .map(|path| format!("`{}`", markdown_code(path)))
                .collect::<Vec<_>>()
                .join(" OR ")
        },
    );
    let arguments = if linked_ir.arguments.is_empty() {
        "-".to_owned()
    } else {
        format!("`{}`", markdown_code(&linked_ir.arguments.join(", ")))
    };
    let semantic = optional_code(linked_ir.semantic_operation.as_deref());
    format!(
        "`{}` `{}`; semantic: {}; {}; IR args: {}",
        markdown_code(&linked_ir.kind),
        markdown_code(&linked_ir.target),
        semantic,
        guards,
        arguments
    )
}

fn write_contexts(
    output: &mut String,
    fact: &FunctionFact,
    reviewed: Option<&ReviewedFunction>,
) -> Result<()> {
    if fact.context_fields.is_empty() {
        return Ok(());
    }
    output.push_str("\n#### Context layout\n\n");
    output.push_str("| Argument | Offset | Width | Access | Mask | Reviewed context/type | Reviewed field | Display type | Description |\n");
    output.push_str("| ---: | ---: | ---: | --- | --- | --- | --- | --- | --- |\n");
    for field in &fact.context_fields {
        let context = reviewed.and_then(|function| {
            function
                .contexts
                .iter()
                .find(|context| context.argument == field.argument)
        });
        let reviewed_field = context.and_then(|context| {
            context.fields.iter().find(|candidate| {
                candidate.offset == field.offset && candidate.width == field.width
            })
        });
        let access = match (field.reads > 0, field.writes > 0) {
            (true, true) => "read/write",
            (true, false) => "read",
            (false, true) => "write",
            (false, false) => "observed",
        };
        writeln!(
            output,
            "| {} | `{:+#x}` | {} | {} ({} / {}) | `{:#010x}` | {} | {} | {} | {} |",
            field.argument,
            field.offset,
            field.width,
            access,
            field.reads,
            field.writes,
            field.write_mask,
            context.map_or_else(
                || "-".to_owned(),
                |context| match (context.name.as_deref(), context.type_name.as_deref()) {
                    (Some(name), Some(type_name)) =>
                        format!("`{}`: `{}`", markdown_code(name), markdown_code(type_name)),
                    (Some(name), None) => format!("`{}`", markdown_code(name)),
                    _ => "-".to_owned(),
                },
            ),
            optional_code(reviewed_field.and_then(|field| field.name.as_deref())),
            optional_code(reviewed_field.and_then(|field| field.display_type.as_deref())),
            reviewed_field
                .and_then(|field| field.description.as_deref())
                .map_or_else(|| "-".to_owned(), markdown_text),
        )
        .expect("writing to String cannot fail");
    }
    Ok(())
}

fn optional_code(value: Option<&str>) -> String {
    value.map_or_else(
        || "-".to_owned(),
        |value| format!("`{}`", markdown_code(value)),
    )
}

fn yes_no(value: bool) -> &'static str {
    if value { "complete" } else { "incomplete" }
}

fn markdown_code(value: &str) -> String {
    value.replace('`', "\\`").replace('|', "\\|")
}

fn markdown_text(value: &str) -> String {
    value.replace('|', "\\|").replace(['\r', '\n'], " ")
}

fn fence_text(value: &str) -> String {
    value.replace("```", "` ` `")
}
