//! Shared acceptance of GTK RX boundaries. A backend fixture captures actual
//! installed identities, counters, hardware writes and delivered events.
//! Delayed DMA/ISR/software RX is injected before each accepted mutation;
//! continuous ingress must remain enabled until its terminal is delivered.

use alloc::vec::Vec;

/// The mutation whose terminal establishes the cut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    InstallBeside,
    Replace,
    SelectRetained,
    SelectCurrent,
}

/// Observation of an actual installed owner, never an expected fixture ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Installation {
    pub identity: usize,
    pub slot: u8,
    pub counter: u64,
}

/// A fact delivered through the backend's public event stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    OldRx,
    Terminal(Installation),
    NewRx,
}

pub struct Trace {
    /// The target before submission for selection, old owner for replacement.
    pub original: Installation,
    /// Slot of the actual owner submitted with the command, including the
    /// vacant slot for InstallBeside. Capture it before transferring ownership.
    pub submitted_slot: u8,
    pub events: Vec<Event>,
    /// Material writes performed by this operation only.
    pub writes: usize,
    pub inactive_material_accepted: bool,
    pub continuous_rx_at_terminal: bool,
}

/// A hardware fixture must inject all ingress stages, not just prepend a
/// synthetic event before a command acknowledgement.
pub trait Harness {
    fn exercise(&mut self, case: Case) -> impl core::future::Future<Output = Trace>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    RxOrder,
    Identity,
    Counter,
    Reinstallation,
    InactiveMaterial,
    UnboundedIngress,
}

pub async fn check(harness: &mut impl Harness) -> Result<(), (Case, Violation)> {
    for case in [
        Case::InstallBeside,
        Case::Replace,
        Case::SelectRetained,
        Case::SelectCurrent,
    ] {
        verify(case, harness.exercise(case).await).map_err(|why| (case, why))?;
    }
    Ok(())
}

fn verify(case: Case, trace: Trace) -> Result<(), Violation> {
    let [
        Event::OldRx,
        Event::OldRx,
        Event::OldRx,
        Event::OldRx,
        Event::Terminal(returned),
        Event::NewRx,
    ] = trace.events.as_slice()
    else {
        return Err(Violation::RxOrder);
    };
    let selected = matches!(case, Case::SelectRetained | Case::SelectCurrent);
    let identity_matches = match case {
        Case::InstallBeside => {
            returned.identity != trace.original.identity
                && trace.submitted_slot != trace.original.slot
        }
        Case::Replace => {
            returned.identity != trace.original.identity
                && trace.submitted_slot == trace.original.slot
        }
        Case::SelectRetained | Case::SelectCurrent => {
            returned.identity == trace.original.identity
                && trace.submitted_slot == trace.original.slot
        }
    };
    if !identity_matches || returned.slot != trace.submitted_slot {
        return Err(Violation::Identity);
    }
    if selected && returned.counter != trace.original.counter {
        return Err(Violation::Counter);
    }
    if trace.writes != usize::from(!selected) {
        return Err(Violation::Reinstallation);
    }
    if trace.inactive_material_accepted {
        return Err(Violation::InactiveMaterial);
    }
    if !trace.continuous_rx_at_terminal {
        return Err(Violation::UnboundedIngress);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_beside_requires_a_new_identity_in_the_submitted_vacant_slot() {
        let original = Installation {
            identity: 7,
            slot: 0,
            counter: 73,
        };
        for (returned, expected) in [
            (original, Err(Violation::Identity)),
            (
                Installation {
                    identity: 8,
                    slot: 0,
                    counter: 0,
                },
                Err(Violation::Identity),
            ),
            (
                Installation {
                    identity: 7,
                    slot: 2,
                    counter: 0,
                },
                Err(Violation::Identity),
            ),
            (
                Installation {
                    identity: 8,
                    slot: 1,
                    counter: 0,
                },
                Err(Violation::Identity),
            ),
            (
                Installation {
                    identity: 8,
                    slot: 2,
                    counter: 0,
                },
                Ok(()),
            ),
        ] {
            let trace = Trace {
                original,
                submitted_slot: 2,
                events: alloc::vec![
                    Event::OldRx,
                    Event::OldRx,
                    Event::OldRx,
                    Event::OldRx,
                    Event::Terminal(returned),
                    Event::NewRx
                ],
                writes: 1,
                inactive_material_accepted: false,
                continuous_rx_at_terminal: true,
            };
            assert_eq!(verify(Case::InstallBeside, trace), expected);
        }
    }
}
