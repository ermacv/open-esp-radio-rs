//! Backward first-definition search. One visited set per location, no recursive traversal.
use blobray_domain::*;
#[derive(Clone, Copy, Debug)]
pub(super) struct Edge {
    pub from: usize,
    pub to: usize,
}
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Step {
    pub definition: bool,
    pub kills: bool,
    pub barrier: bool,
}
pub(super) struct Reaching<'m> {
    pub incoming: bool,
    pub definitions: AdmittedVec<'m, usize>,
    pub barriers: AdmittedVec<'m, usize>,
    /// Parent `steps.len()` is the virtual point immediately before the anchor.
    pub parents: AdmittedVec<'m, Option<usize>>,
}
pub(super) fn search<'m>(
    edges: &[Edge],
    steps: &[Step],
    reachable: &[bool],
    entry: usize,
    anchor: usize,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Reaching<'m>> {
    if entry >= steps.len()
        || anchor >= steps.len()
        || reachable.len() != steps.len()
        || !reachable[anchor]
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "publication anchor is not reachable in the saved CFG",
        ));
    }
    for (i, edge) in edges.iter().enumerate() {
        c.checkpoint(1)?;
        if edge.from >= steps.len() || edge.to >= steps.len() || i > 0 && edges[i - 1].to > edge.to
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "invalid reverse instruction graph",
            ));
        }
    }
    let mut out = Reaching {
        incoming: entry == anchor,
        definitions: AdmittedVec::new(memory),
        barriers: AdmittedVec::new(memory),
        parents: AdmittedVec::new(memory),
    };
    for _ in steps {
        c.checkpoint(1)?;
        out.parents.push(None, c.position())?;
    }
    let mut queue = AdmittedVec::new(memory);
    let mut enqueue = |node: usize,
                       parent: usize,
                       out: &mut Reaching<'m>,
                       c: &mut dyn RunControl|
     -> Result<()> {
        c.checkpoint(edges.len().max(1).ilog2() as u64 + 1)?;
        let first = edges.partition_point(|e| e.to < node);
        for edge in edges[first..].iter().take_while(|e| e.to == node) {
            c.checkpoint(1)?;
            if reachable[edge.from] && out.parents[edge.from].is_none() {
                out.parents[edge.from] = Some(parent);
                queue.push(edge.from, c.position())?;
            }
        }
        Ok(())
    };
    enqueue(anchor, steps.len(), &mut out, c)?;
    // Release the queue borrow between expansion steps.
    let mut head = 0;
    while head < queue.len() {
        c.checkpoint(1)?;
        let node = queue[head];
        head += 1;
        let step = steps[node];
        if step.definition {
            out.definitions.push(node, c.position())?;
        }
        if step.barrier {
            out.barriers.push(node, c.position())?;
        }
        if step.kills {
            continue;
        }
        out.incoming |= node == entry;
        c.checkpoint(edges.len().max(1).ilog2() as u64 + 1)?;
        let first = edges.partition_point(|e| e.to < node);
        for edge in edges[first..].iter().take_while(|e| e.to == node) {
            c.checkpoint(1)?;
            if reachable[edge.from] && out.parents[edge.from].is_none() {
                out.parents[edge.from] = Some(node);
                queue.push(edge.from, c.position())?;
            }
        }
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn run(
        edges: &[(usize, usize)],
        steps: &[Step],
        entry: usize,
        anchor: usize,
    ) -> (bool, Vec<usize>, Vec<usize>) {
        let memory = WorkingMemory::new(65536).unwrap();
        let mut edges: Vec<_> = edges
            .iter()
            .map(|(from, to)| Edge {
                from: *from,
                to: *to,
            })
            .collect();
        edges.sort_by_key(|e| (e.to, e.from));
        let found = search(
            &edges,
            steps,
            &vec![true; steps.len()],
            entry,
            anchor,
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        let mut defs = found.definitions.to_vec();
        defs.sort_unstable();
        let mut barriers = found.barriers.to_vec();
        barriers.sort_unstable();
        let result = (found.incoming, defs, barriers);
        drop(found);
        assert_eq!(memory.used(), 0);
        result
    }
    #[test]
    fn joins_kills_and_clobbers_keep_last_definitions_without_path_enumeration() {
        let none = Step::default();
        let write = Step {
            definition: true,
            kills: true,
            barrier: false,
        };
        let clobber = Step {
            barrier: true,
            ..none
        };
        let diamond = [(0, 1), (0, 2), (1, 3), (2, 3), (3, 4)];
        assert_eq!(
            run(&diamond, &[none, write, write, none, none], 0, 4),
            (false, vec![1, 2], vec![])
        );
        assert_eq!(
            run(&diamond, &[none, write, none, none, none], 0, 4),
            (true, vec![1], vec![])
        );
        assert_eq!(
            run(&diamond, &[none, write, write, write, none], 0, 4),
            (false, vec![3], vec![])
        );
        assert_eq!(
            run(&diamond, &[none, write, write, clobber, none], 0, 4),
            (false, vec![1, 2], vec![3])
        );
        assert_eq!(
            run(&[(0, 1), (1, 2)], &[clobber, write, none], 0, 2),
            (false, vec![1], vec![])
        );
    }
    #[test]
    fn loop_can_reach_an_earlier_anchor_iteration_and_conditional_write_keeps_input() {
        let none = Step::default();
        let maybe = Step {
            definition: true,
            ..none
        };
        let write = Step {
            definition: true,
            kills: true,
            barrier: false,
        };
        assert_eq!(
            run(&[(0, 1), (1, 2), (2, 1)], &[none, write, none], 0, 1),
            (true, vec![1], vec![])
        );
        assert_eq!(
            run(&[(0, 1), (1, 2)], &[none, maybe, none], 0, 2),
            (true, vec![1], vec![])
        );
    }
}
