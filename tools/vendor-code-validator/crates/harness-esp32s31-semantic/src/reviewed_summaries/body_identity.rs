//! Exact reviewed symbol identity matching.

pub(super) struct ReviewedBodyIdentity<'a> {
    pub(super) name: &'a str,
    pub(super) address: u64,
    pub(super) size: usize,
}

pub(super) fn reviewed_identity_matches(
    actual: ReviewedBodyIdentity<'_>,
    expected: ReviewedBodyIdentity<'_>,
) -> bool {
    actual.name == expected.name
        && actual.address == expected.address
        && actual.size == expected.size
}
