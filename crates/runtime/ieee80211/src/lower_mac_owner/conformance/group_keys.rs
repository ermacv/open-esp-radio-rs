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
    if selected && returned.identity != trace.original.identity
        || case == Case::Replace && returned.identity == trace.original.identity
        || case != Case::InstallBeside && returned.slot != trace.original.slot
    {
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
