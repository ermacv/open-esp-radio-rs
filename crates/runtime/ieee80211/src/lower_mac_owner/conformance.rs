//! Backend-independent TX ownership scenarios. The model runs these now;
//! S31 must supply the same harness when its port runtime is rewritten.
//! Harnesses submit the operations in `OPERATIONS`, retain the original
//! identities in opaque contexts, and report identities of actual returned
//! resources. No observation may be synthesized from the expected identity.

use alloc::vec::Vec;
use oer_ieee80211_mac::qos::WmmAccessCategory;

/// Resource classes deliberately sharing an AC queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    Small,
    Large,
}

/// One common scenario operation, in submission order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Operation {
    /// Context identity, independent of frame contents.
    pub id: u8,
    /// Buffer pool.
    pub class: Class,
    /// Hardware/software queue.
    pub category: WmmAccessCategory,
}

/// Alternating classes, same-AC order, and other ACs completing first.
pub const OPERATIONS: [Operation; 4] = [
    Operation {
        id: 0,
        class: Class::Small,
        category: WmmAccessCategory::BestEffort,
    },
    Operation {
        id: 1,
        class: Class::Large,
        category: WmmAccessCategory::BestEffort,
    },
    Operation {
        id: 2,
        class: Class::Small,
        category: WmmAccessCategory::Voice,
    },
    Operation {
        id: 3,
        class: Class::Large,
        category: WmmAccessCategory::Video,
    },
];

/// How the harness terminates its operations. BestEffort is held while
/// other ACs complete; then it is released, withdrawn, or ended by teardown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scenario {
    Complete,
    CloseGate,
    Disable,
    Refuse,
    Poison,
}

/// Stable identities, observed from the resources themselves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Resources {
    /// Ticket slot.
    pub ticket: u8,
    /// Buffer slot (class remains part of identity).
    pub buffer: u8,
    /// Pool class.
    pub class: Class,
    /// External body allocation identity, including after Sent renewal.
    pub body: usize,
}

/// Distinguishes admission refusal from an accepted terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum End {
    Completed,
    Withdrawn,
    Refused,
}

/// An observation of an actual return, including the original opaque context.
pub struct Returned {
    /// Original operation from the returned context.
    pub operation: Operation,
    /// Resource identity recorded before transfer, carried by that context.
    pub expected: Resources,
    /// Identity read from the returned owners.
    pub actual: Resources,
    /// Actual API path taken by the return.
    pub end: End,
}

/// Evidence after the install has ended.
pub struct Trace {
    /// Actual returns in observation order.
    pub returned: Vec<Returned>,
    /// Context destructors run, including fail-stop-owned contexts.
    pub contexts_dropped: usize,
    /// Publications observed by the backend fixture.
    pub publications: usize,
}

/// Backend-specific execution, shared scenarios and checks.
pub trait Harness {
    /// Exercise the prescribed operations through the real backend API.
    fn exercise(&mut self, scenario: Scenario) -> impl core::future::Future<Output = Trace>;
}

/// The common suite found a contract violation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    Owners,
    Duplicate,
    Order,
    Missing,
    TerminalKind,
    Lifetime,
    Publication,
}

/// Run the same ownership suite for any backend fixture. Refusal creates
/// no publication or terminal; fail-stop ends all owners with the install.
pub async fn check(harness: &mut impl Harness) -> Result<(), (Scenario, Violation)> {
    for scenario in [
        Scenario::Complete,
        Scenario::CloseGate,
        Scenario::Disable,
        Scenario::Refuse,
        Scenario::Poison,
    ] {
        verify(scenario, harness.exercise(scenario).await).map_err(|why| (scenario, why))?;
    }
    Ok(())
}

fn verify(scenario: Scenario, trace: Trace) -> Result<(), Violation> {
    if trace.contexts_dropped != OPERATIONS.len() {
        return Err(Violation::Lifetime);
    }
    let expected_publications = match scenario {
        Scenario::Complete => 4,
        Scenario::Refuse => 0,
        _ => 2,
    };
    if trace.publications != expected_publications {
        return Err(Violation::Publication);
    }
    let mut remaining = OPERATIONS.map(Some);
    for returned in trace.returned {
        let Some(slot) = remaining
            .iter_mut()
            .find(|op| op.as_ref() == Some(&returned.operation))
        else {
            return Err(Violation::Duplicate);
        };
        *slot = None;
        if returned.expected != returned.actual || returned.actual.class != returned.operation.class
        {
            return Err(Violation::Owners);
        }
        if remaining.iter().flatten().any(|earlier| {
            earlier.category == returned.operation.category && earlier.id < returned.operation.id
        }) {
            return Err(Violation::Order);
        }
        let expected_end = match (scenario, returned.operation.category) {
            (Scenario::Refuse, _) => End::Refused,
            (Scenario::CloseGate | Scenario::Disable, WmmAccessCategory::BestEffort) => {
                End::Withdrawn
            }
            _ => End::Completed,
        };
        if returned.end != expected_end {
            return Err(Violation::TerminalKind);
        }
    }
    if scenario == Scenario::Poison {
        if remaining.iter().flatten().count() != 2
            || remaining
                .iter()
                .flatten()
                .any(|op| op.category != WmmAccessCategory::BestEffort)
        {
            return Err(Violation::Missing);
        }
    } else if remaining.iter().any(Option::is_some) {
        return Err(Violation::Missing);
    }
    Ok(())
}
