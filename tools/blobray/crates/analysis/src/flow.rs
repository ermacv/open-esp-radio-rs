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
