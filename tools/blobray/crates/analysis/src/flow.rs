//! Iterative reachability over the caller's selected, unambiguous graph.
use blobray_domain::*;
#[derive(Clone, Copy, Debug)]
pub struct Arc {
    pub from: usize,
    pub to: usize,
    pub record: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reached {
    pub depth: u32,
    pub parent: Option<usize>,
}
/// Propagate at most 32 profile memberships through an already resolved graph.
/// Labels outside `following` remain roots only. A queued node owns one work item;
/// cycles terminate because labels only grow within this finite bitset.
pub fn propagate_profiles(
    labels: &mut [u32],
    arcs: &[Arc],
    following: u32,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    for (i, edge) in arcs.iter().enumerate() {
        c.checkpoint(1)?;
        if edge.from >= labels.len()
            || edge.to >= labels.len()
            || i > 0 && arcs[i - 1].from > edge.from
        {
            return Err(Error::new(ErrorCode::Integrity, "invalid profile graph"));
        }
    }
    let mut pending = AdmittedVec::new(memory);
    let mut queued = AdmittedVec::new(memory);
    for (i, label) in labels.iter().enumerate() {
        c.checkpoint(1)?;
        let active = label & following != 0;
        queued.push(active, c.position())?;
        if active {
            pending.push(i, c.position())?;
        }
    }
    while let Some(node) = pending.pop() {
        c.checkpoint(arcs.len().max(1).ilog2() as u64 + 1)?;
        queued[node] = false;
        let added = labels[node] & following;
        let start = arcs.partition_point(|edge| edge.from < node);
        for edge in arcs[start..].iter().take_while(|edge| edge.from == node) {
            c.checkpoint(1)?;
            if labels[edge.to] | added != labels[edge.to] {
                labels[edge.to] |= added;
                if !queued[edge.to] {
                    pending.push(edge.to, c.position())?;
                    queued[edge.to] = true;
                }
            }
        }
    }
    Ok(())
}
pub fn reachable<'m>(
    nodes: usize,
    arcs: &[Arc],
    root: usize,
    max_depth: u32,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<AdmittedVec<'m, Option<Reached>>> {
    c.checkpoint(1)?;
    if root >= nodes {
        return Err(Error::new(ErrorCode::Integrity, "flow root is absent"));
    }
    for (i, arc) in arcs.iter().enumerate() {
        c.checkpoint(1)?;
        if arc.from >= nodes || arc.to >= nodes || i > 0 && arcs[i - 1].from > arc.from {
            return Err(Error::new(
                ErrorCode::Integrity,
                "invalid selected flow graph",
            ));
        }
    }
    let mut reached = AdmittedVec::new(memory);
    for _ in 0..nodes {
        reached.push(None, c.position())?;
    }
    let mut queue = AdmittedVec::new(memory);
    reached[root] = Some(Reached {
        depth: 0,
        parent: None,
    });
    queue.push(root, c.position())?;
    let mut head = 0;
    while head < queue.len() {
        c.checkpoint(1)?;
        let node = queue[head];
        head += 1;
        let depth = reached[node].unwrap().depth;
        if depth == max_depth {
            continue;
        }
        c.checkpoint(arcs.len().max(1).ilog2() as u64 + 1)?;
        let start = arcs.partition_point(|a| a.from < node);
        for (i, arc) in arcs
            .iter()
            .enumerate()
            .skip(start)
            .take_while(|(_, a)| a.from == node)
        {
            c.checkpoint(1)?;
            if reached[arc.to].is_none() {
                reached[arc.to] = Some(Reached {
                    depth: depth + 1,
                    parent: Some(i),
                });
                queue.push(arc.to, c.position())?;
            }
        }
    }
    Ok(reached)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_membership_closes_diamond_cycle_but_keeps_nonfollowing_roots_local() {
        let m = WorkingMemory::new(8192).unwrap();
        let arcs: Vec<_> = [(0, 1), (0, 2), (1, 3), (2, 3), (3, 0)]
            .into_iter()
            .map(|(from, to)| Arc {
                from,
                to,
                record: 0,
            })
            .collect();
        let mut labels = [3, 0, 4, 0];
        propagate_profiles(&mut labels, &arcs, 1 | 4, &m, &mut || Ok(())).unwrap();
        assert_eq!(labels, [7, 5, 5, 5]);
        assert_eq!(m.used(), 0);
        let m = WorkingMemory::new(1).unwrap();
        assert_eq!(
            propagate_profiles(&mut [1; 4], &arcs, 1, &m, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimited
        );
        assert_eq!(m.used(), 0);
        assert_eq!(
            propagate_profiles(&mut [1], &arcs, 1, &m, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
    }
    #[test]
    fn diamond_cycle_and_depth_limit_have_bounded_shortest_witnesses() {
        let m = WorkingMemory::new(8192).unwrap();
        let arcs = [
            Arc {
                from: 0,
                to: 1,
                record: 1,
            },
            Arc {
                from: 0,
                to: 2,
                record: 2,
            },
            Arc {
                from: 1,
                to: 3,
                record: 3,
            },
            Arc {
                from: 2,
                to: 3,
                record: 4,
            },
            Arc {
                from: 3,
                to: 0,
                record: 5,
            },
        ];
        let found = reachable(4, &arcs, 0, 100, &m, &mut || Ok(())).unwrap();
        assert_eq!(
            found[3],
            Some(Reached {
                depth: 2,
                parent: Some(2)
            })
        );
        assert_eq!(
            found[0],
            Some(Reached {
                depth: 0,
                parent: None
            })
        );
        drop(found);
        assert_eq!(m.used(), 0);
        let limited = reachable(4, &arcs, 0, 1, &m, &mut || Ok(())).unwrap();
        assert!(limited[3].is_none());
        drop(limited);
        assert!(
            matches!(reachable(4,&arcs,4,1,&m,&mut || Ok(())),Err(e) if e.code==ErrorCode::Integrity)
        );
        let m = WorkingMemory::new(1).unwrap();
        assert!(
            matches!(reachable(4,&arcs,0,1,&m,&mut || Ok(())),Err(e) if e.code==ErrorCode::ResourceLimited)
        );
        assert_eq!(m.used(), 0);
    }
}
