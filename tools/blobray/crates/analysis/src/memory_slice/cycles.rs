//! Iterative SCC membership used only to identify values computed at most once.
use super::*;
pub(super) fn identify<'m>(
    nodes: usize,
    forward: &[crate::flow::Arc],
    reverse: &[reaching::Edge],
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<AdmittedVec<'m, bool>> {
    let mut seen = AdmittedVec::new(memory);
    let mut assigned = AdmittedVec::new(memory);
    let mut cyclic = AdmittedVec::new(memory);
    for _ in 0..nodes {
        c.checkpoint(1)?;
        seen.push(false, c.position())?;
        assigned.push(false, c.position())?;
        cyclic.push(false, c.position())?;
    }
    let mut order = AdmittedVec::new(memory);
    let mut frames = AdmittedVec::new(memory);
    for root in 0..nodes {
        c.checkpoint(1)?;
        if seen[root] {
            continue;
        }
        seen[root] = true;
        c.checkpoint(forward.len().max(1).ilog2() as u64 + 1)?;
        frames.push(
            (root, forward.partition_point(|e| e.from < root)),
            c.position(),
        )?;
        while !frames.is_empty() {
            c.checkpoint(1)?;
            let top = frames.len() - 1;
            let (node, next) = frames[top];
            if let Some(edge) = forward.get(next).filter(|e| e.from == node) {
                frames[top].1 += 1;
                if !seen[edge.to] {
                    seen[edge.to] = true;
                    c.checkpoint(forward.len().max(1).ilog2() as u64 + 1)?;
                    frames.push(
                        (edge.to, forward.partition_point(|e| e.from < edge.to)),
                        c.position(),
                    )?;
                }
            } else {
                frames.pop();
                order.push(node, c.position())?;
            }
        }
    }
    drop(frames);
    drop(seen);
    let mut stack = AdmittedVec::new(memory);
    let mut component = AdmittedVec::new(memory);
    for &root in order.iter().rev() {
        c.checkpoint(1)?;
        if assigned[root] {
            continue;
        }
        assigned[root] = true;
        stack.push(root, c.position())?;
        let mut self_edge = false;
        while let Some(node) = stack.pop() {
            c.checkpoint(1)?;
            component.push(node, c.position())?;
            c.checkpoint(reverse.len().max(1).ilog2() as u64 + 1)?;
            let first = reverse.partition_point(|e| e.to < node);
            for edge in reverse[first..].iter().take_while(|e| e.to == node) {
                c.checkpoint(1)?;
                self_edge |= edge.from == node;
                if !assigned[edge.from] {
                    assigned[edge.from] = true;
                    stack.push(edge.from, c.position())?;
                }
            }
        }
        let cycle = component.len() > 1 || self_edge;
        while let Some(node) = component.pop() {
            cyclic[node] = cycle;
        }
    }
    Ok(cyclic)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_cycle_members_repeat_not_their_successors() {
        let m = WorkingMemory::new(65536).unwrap();
        let forward: Vec<_> = [(0, 1), (1, 2), (2, 1), (2, 3), (3, 4), (4, 4)]
            .into_iter()
            .map(|(from, to)| crate::flow::Arc {
                from,
                to,
                record: 0,
            })
            .collect();
        let mut reverse: Vec<_> = forward
            .iter()
            .map(|e| reaching::Edge {
                from: e.from,
                to: e.to,
            })
            .collect();
        reverse.sort_by_key(|e| (e.to, e.from));
        let result = identify(5, &forward, &reverse, &m, &mut || Ok(())).unwrap();
        assert_eq!(&*result, &[false, true, true, false, true]);
        drop(result);
        assert_eq!(m.used(), 0);
    }
}
