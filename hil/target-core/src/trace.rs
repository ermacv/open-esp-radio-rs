//! The HIL platform's own trace events: the triggers that freeze the trace
//! when a boot ends badly. They carry no words; the post-mortem record holds
//! the details, and the trace holds what happened before.

use oer_trace::{Channel, Domain, Event, Kind};

/// The hang watchdog found a stall and is about to reset the chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hang;

/// A panic is being handled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Panic;

impl Event for Hang {
    const KIND: Kind = Kind::new(Domain::Platform, 1);
    const CHANNEL: Channel = Channel::new(Domain::Platform, 0);

    fn encode(&self) -> [u32; 2] {
        [0; 2]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        (words == [0; 2]).then_some(Self)
    }
}

impl Event for Panic {
    const KIND: Kind = Kind::new(Domain::Platform, 2);
    const CHANNEL: Channel = Channel::new(Domain::Platform, 0);

    fn encode(&self) -> [u32; 2] {
        [0; 2]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        (words == [0; 2]).then_some(Self)
    }
}

/// The name of a platform trace kind, for the host's decoded trace.
pub fn platform_name(kind: Kind) -> Option<&'static str> {
    if kind == Hang::KIND {
        Some("platform.hang")
    } else if kind == Panic::KIND {
        Some("platform.panic")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_triggers_have_distinct_named_kinds() {
        assert_ne!(Hang::KIND, Panic::KIND);
        assert_eq!(platform_name(Hang::KIND), Some("platform.hang"));
        assert_eq!(platform_name(Panic::KIND), Some("platform.panic"));
        assert_eq!(Hang::decode(Hang.encode()), Some(Hang));
    }
}
