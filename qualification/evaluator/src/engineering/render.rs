//! Human projections of the same engineering map used by JSON consumers.

use super::*;

const CONTRACT: &str = "Implementation and host coverage are reviewed declarations. Knowledge links do not establish completeness. Saved observations retain the evaluator's applicability decisions. Dependency closure is context, not a rerun plan. Actions are explained candidates, not a priority ranking or an execution queue.";

pub(super) fn print(map: &ProjectMap, next_only: bool, details: bool) {
    println!(
        "PROJECT\tmode={}\ttarget={}\tfocus={}\tentries={}\tactions={}",
        map.mode,
        map.target.as_deref().unwrap_or("catalog"),
        map.focus.as_deref().unwrap_or("all"),
        map.entries.len(),
        map.next.len()
    );
    println!("NOTE\t{CONTRACT}");
    if !next_only {
        for entry in &map.entries {
            println!(
                "STATE\t{}:{}\timplementation={}\tknowledge={}\thost={}\tasync={}\tvendor={}\thil={}",
                entry.kind,
                entry.id,
                entry.implementation,
                entry.knowledge_status,
                entry.host_declaration.as_deref().unwrap_or("not-declared"),
                entry.async_declaration.as_deref().unwrap_or("not-declared"),
                entry
                    .evidence
                    .as_ref()
                    .map_or("not-evaluated", |e| e.vendor),
                entry.evidence.as_ref().map_or("not-evaluated", |e| e.hil)
            );
            if let Some(evidence) = &entry.evidence {
                let counts = &evidence.hil_observations;
                println!(
                    "OBSERVATIONS\t{}\ttotal={}\tpassed={}\tfailed={}\texcluded={}",
                    entry.id, counts.total, counts.passed, counts.failed, counts.excluded
                );
            }
            if !details {
                continue;
            }
            println!("SCOPE\t{}\t{}", entry.id, entry.scope);
            println!(
                "DEPENDENCIES\t{}\t{}",
                entry.id,
                entry.dependencies.join(",")
            );
            for (axis, reason) in &entry.not_applicable {
                println!("NOT-APPLICABLE\t{}\t{axis}\t{reason}", entry.id);
            }
            for reference in &entry.vendor_roots {
                println!(
                    "VENDOR-ROOT\t{}\t{}\t{}",
                    entry.id, reference.source, reference.symbol
                );
            }
            for limit in &entry.limits {
                println!("LIMIT\t{}\t{limit}", entry.id);
            }
            for path in &entry.owners {
                println!("OWNER\t{}\t{}", entry.id, path.display());
            }
            for path in &entry.knowledge {
                println!("KNOWLEDGE\t{}\t{}", entry.id, path.display());
            }
            for test in &entry.development.host_tests {
                println!(
                    "HOST-TEST\t{}\tmanifest={}\tfilter={}\tsource={}\texecution=not-recorded",
                    entry.id,
                    test.manifest.display(),
                    test.filter,
                    test.source.display()
                );
            }
            for path in &entry.hil_reviews {
                println!("HIL-REVIEW\t{}\t{}", entry.id, path.display());
            }
            for requirement in &entry.hil_requirements {
                println!(
                    "HIL-REQUIREMENT\t{}\tscenario={}\tchecks={}\trepetitions={}",
                    entry.id,
                    requirement.scenario,
                    requirement.checks.join(","),
                    requirement.minimum_repetitions
                );
            }
            if let Some(evidence) = &entry.evidence {
                for decision in &evidence.hil_decisions {
                    println!(
                        "HIL-DECISION\t{}\t{}",
                        entry.id,
                        serde_json::to_string(decision).expect("serializable evidence decision")
                    );
                }
            }
        }
    }
    if !next_only && !details {
        println!(
            "DETAILS\tUse status --details for scopes, limits, links and observations; next for work reasons. JSON always contains the full selected map."
        );
    }
    for action in map.next.iter().filter(|_| next_only || details) {
        println!(
            "NEXT\t{}:{}\tkind={}\tsubject={}\torigin={}\t{}",
            action.entry_kind,
            action.entry,
            action.kind.label(),
            action.subject,
            action.origin,
            action.reason
        );
    }
    for project in &map.research_projects {
        println!(
            "RESEARCH\t{}\tinspect existing Blobray project status / project research next",
            project.display()
        );
    }
}

pub(super) fn markdown(map: &ProjectMap, output: &Path, root: &Path) -> Result<String> {
    let mut text = format!(
        "# Project engineering map\n\nMode: `{}`. Target: `{}`. Focus: `{}`.\n\n{CONTRACT}\n\n",
        map.mode,
        map.target.as_deref().unwrap_or("catalog"),
        map.focus.as_deref().unwrap_or("all")
    );
    if let Some(commit) = &map.repository_commit {
        text.push_str(&format!(
            "Repository: `{commit}`; dirty: `{}`.\n\n",
            map.repository_dirty.unwrap_or(false)
        ));
    }
    text.push_str("## Inputs\n\n");
    for input in &map.sources {
        text.push_str(&format!(
            "- [{}]({}) (`{}`)\n",
            input.id,
            crate::inventory::link_to(&input.path, output, root)?,
            input.sha256
        ));
    }
    text.push_str("\n## State\n\n| Kind / identity | Implementation | Knowledge links | Host declaration | Vendor evidence | HIL evidence |\n| --- | --- | --- | --- | --- | --- |\n");
    for entry in &map.entries {
        text.push_str(&format!(
            "| {} / {} | {} | {} | {} | {} | {} |\n",
            entry.kind,
            entry.id,
            entry.implementation,
            entry.knowledge_status,
            entry.host_declaration.as_deref().unwrap_or("not-declared"),
            entry
                .evidence
                .as_ref()
                .map_or("not-evaluated", |e| e.vendor),
            entry.evidence.as_ref().map_or("not-evaluated", |e| e.hil)
        ));
    }
    for entry in &map.entries {
        text.push_str(&format!(
            "\n## {}: {}\n\n{}\n\n{}\n\n",
            entry.kind,
            entry.id,
            prose(&entry.title, entry, output, root)?,
            prose(&entry.scope, entry, output, root)?
        ));
        if !entry.dependencies.is_empty() {
            text.push_str(&format!(
                "Dependencies: `{}`.\n\n",
                entry.dependencies.join("`, `")
            ));
        }
        if !entry.source_facts.is_empty() {
            text.push_str(&format!(
                "Source facts: `{}`.\n\n",
                entry.source_facts.join("`, `")
            ));
        }
        for limit in &entry.limits {
            text.push_str(&format!("Limit: {}\n\n", escape(limit)));
        }
        for (label, paths) in [("Owner", &entry.owners), ("Knowledge", &entry.knowledge)] {
            for path in paths {
                text.push_str(&format!(
                    "- {label}: [{}]({})\n",
                    path.display(),
                    crate::inventory::link_to(path, output, root)?
                ));
            }
            text.push('\n');
        }
        for test in &entry.development.host_tests {
            text.push_str(&format!("- Host test: [{}]({}), filter `{}`; [source]({}). Execution is not recorded by this declaration.\n", test.manifest.display(), crate::inventory::link_to(&test.manifest, output, root)?, escape(&test.filter), crate::inventory::link_to(&test.source, output, root)?));
        }
        text.push('\n');
        for path in &entry.hil_reviews {
            text.push_str(&format!(
                "- Applicability review: [{}]({})\n",
                path.display(),
                crate::inventory::link_to(path, output, root)?
            ));
        }
        for requirement in &entry.hil_requirements {
            text.push_str(&format!(
                "- HIL: `{}`, repetitions {}, checks `{}`.\n",
                requirement.scenario,
                requirement.minimum_repetitions,
                requirement.checks.join("`, `")
            ));
        }
        for reference in &entry.vendor_roots {
            text.push_str(&format!(
                "- Vendor root: `{}` / `{}`.\n",
                reference.source,
                escape(&reference.symbol)
            ));
        }
        for (axis, reason) in &entry.not_applicable {
            text.push_str(&format!("- {axis} not applicable: {}.\n", escape(reason)));
        }
        if let Some(evidence) = &entry.evidence {
            text.push_str("\nSaved evidence and applicability:\n\n```json\n");
            text.push_str(&serde_json::to_string_pretty(evidence)?);
            text.push_str("\n```\n");
        }
        text.push_str("\nNext work:\n\n");
        for action in map
            .next
            .iter()
            .filter(|a| a.entry == entry.id && a.entry_kind == entry.kind)
        {
            text.push_str(&format!(
                "- **{}** — `{}`: {} ({}).\n",
                action.kind.label(),
                action.subject,
                escape(&action.reason),
                action.origin
            ));
        }
    }
    text.push_str("\n## Research owners\n\nUse the existing Blobray project status and research-next views for the detailed research backlog.\n\n");
    for project in &map.research_projects {
        text.push_str(&format!(
            "- [{}]({})\n",
            project.display(),
            crate::inventory::link_to(project, output, root)?
        ));
    }
    Ok(text)
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('`', "&#96;")
}

fn prose(value: &str, entry: &Entry, output: &Path, root: &Path) -> Result<String> {
    if let Some(source) = &entry.documentation_base {
        crate::inventory::rewrite_links(value, source, output, root)
    } else {
        Ok(escape(value))
    }
}
