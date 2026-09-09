//! Atomic recovery of the two disjoint ISR owners.

/// Called with the ISR storage lock held. Missing storage must not disable
/// routing or consume the other owner; successful recovery first excludes all
/// handlers and only then removes their register capabilities.
pub(super) fn detach_pair<A, B>(
    first: &mut Option<A>,
    second: &mut Option<B>,
    disable: impl FnOnce(),
) -> Option<(A, B)> {
    if first.is_none() || second.is_none() {
        return None;
    }
    disable();
    Some((
        first.take().expect("checked first owner"),
        second.take().expect("checked second owner"),
    ))
}
