//! Explicit profile roots and finite call closure over the common resolver's graph.
use super::*;
use blobray_analysis::flow::{Arc, propagate_profiles};

pub(super) fn select(
    profiles: &[IrProfile],
    nodes: &mut [Node<'_>],
    calls: &[Call<'_>],
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<IrProfileSummary>> {
    let mut following = 0;
    for (i, profile) in profiles.iter().enumerate() {
        let bit = 1 << i;
        if profile.include_reachable {
            following |= bit;
        }
        match &profile.roots {
            IrRoots::Analyses { analyses } => {
                for id in analyses {
                    let n = position(nodes, id, c)?;
                    nodes[n].roots |= bit;
                }
            }
            IrRoots::All => {
                for node in nodes.iter_mut() {
                    c.checkpoint(1)?;
                    node.roots |= bit;
                }
            }
            IrRoots::NamePrefix { prefix } => {
                for node in nodes.iter_mut() {
                    c.checkpoint(1)?;
                    match &node.name {
                        Some(name) if name.starts_with(prefix) => node.roots |= bit,
                        None if node.function.location.selector.symbol().is_some() => {
                            return Err(invalid(
                                "prefix roots require selected publication name metadata for every named function",
                            ));
                        }
                        _ => (),
                    }
                }
            }
        }
        if !nodes.iter().any(|n| n.roots & bit != 0) {
            return Err(invalid("IR profile has no roots"));
        }
    }
    let mut arcs = AdmittedVec::new(memory);
    for call in calls {
        c.checkpoint(1)?;
        if let NavigationRecord::Call {
            caller,
            candidates,
            issue: None,
            record,
            ..
        } = &call.record
            && let [target] = candidates.as_slice()
        {
            arcs.push(
                Arc {
                    from: position(nodes, &caller.analysis, c)?,
                    to: position(nodes, &target.analysis, c)?,
                    record: *record,
                },
                c.position(),
            )?;
        }
    }
    c.checkpoint(arcs.len() as u64 * (arcs.len().max(1).ilog2() as u64 + 1))?;
    arcs.sort_unstable_by_key(|edge| (edge.from, edge.to, edge.record));
    let mut labels = AdmittedVec::new(memory);
    for node in nodes.iter() {
        labels.push(node.roots, c.position())?;
    }
    propagate_profiles(&mut labels, &arcs, following, memory, c)?;
    for (node, &label) in nodes.iter_mut().zip(labels.iter()) {
        node.profiles = label;
    }
    let mut summary: Vec<_> = profiles
        .iter()
        .map(|p| IrProfileSummary {
            name: p.name.clone(),
            roots: 0,
            functions: 0,
            partial_functions: 0,
            unresolved_links: 0,
        })
        .collect();
    for node in nodes.iter() {
        c.checkpoint(profiles.len() as u64)?;
        for (i, p) in summary.iter_mut().enumerate() {
            p.roots += u64::from(node.roots & (1 << i) != 0);
            if node.profiles & (1 << i) != 0 {
                p.functions += 1;
                p.partial_functions += u64::from(
                    !node.manifest.coverage.complete()
                        || node.manifest.semantics.is_none_or(|s| !s.complete),
                );
            }
        }
    }
    for call in calls {
        if let NavigationRecord::Call {
            caller,
            issue,
            candidates,
            ..
        } = &call.record
            && (issue.is_some() || candidates.len() != 1)
        {
            let mask = nodes[position(nodes, &caller.analysis, c)?].profiles;
            c.checkpoint(profiles.len() as u64)?;
            for (i, p) in summary.iter_mut().enumerate() {
                p.unresolved_links += u64::from(mask & (1 << i) != 0);
            }
        }
    }
    Ok(summary)
}
