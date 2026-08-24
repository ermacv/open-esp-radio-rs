//! Relationship between physical register definitions and observed bus widths.
//!
//! Discovery records the width of each load/store. The reviewed model records
//! one non-overlapping physical register width. Those are deliberately not the
//! same identity: a halfword access can address either half of one reviewed
//! 32-bit register without creating a second physical register.

use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn physical_register_identity(
    identities: &BTreeMap<(u64, u32), String>,
    address: u64,
    access_width: u32,
) -> Option<(&(u64, u32), &String)> {
    let access_end = address.checked_add(u64::from(access_width).div_ceil(8))?;
    identities.iter().find(|((start, register_width), _)| {
        let Some(register_end) = start.checked_add(u64::from(*register_width).div_ceil(8)) else {
            return false;
        };
        *start <= address && access_end <= register_end
    })
}

pub(crate) fn observation_is_reviewed(
    identities: &BTreeMap<(u64, u32), String>,
    observation: &(u64, u32),
) -> bool {
    physical_register_identity(identities, observation.0, observation.1).is_some()
}

pub(crate) fn physical_register_is_observed(
    register: &(u64, u32),
    observations: &BTreeSet<(u64, u32)>,
) -> bool {
    let Some(register_end) = register.0.checked_add(u64::from(register.1).div_ceil(8)) else {
        return false;
    };
    observations.iter().any(|(address, access_width)| {
        let Some(access_end) = address.checked_add(u64::from(*access_width).div_ceil(8)) else {
            return false;
        };
        register.0 <= *address && access_end <= register_end
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subword_accesses_resolve_to_one_physical_register() {
        let identities = BTreeMap::from([((0x1000, 32), "RADIO.CONTROL".to_owned())]);

        assert_eq!(
            physical_register_identity(&identities, 0x1000, 16).map(|(_, name)| name.as_str()),
            Some("RADIO.CONTROL")
        );
        assert_eq!(
            physical_register_identity(&identities, 0x1002, 16).map(|(_, name)| name.as_str()),
            Some("RADIO.CONTROL")
        );
        assert!(physical_register_identity(&identities, 0x1001, 32).is_none());
        assert!(physical_register_identity(&identities, 0x1004, 8).is_none());
    }

    #[test]
    fn one_access_cannot_span_adjacent_physical_registers() {
        let identities = BTreeMap::from([
            ((0x1000, 16), "RADIO.LOW".to_owned()),
            ((0x1002, 16), "RADIO.HIGH".to_owned()),
        ]);

        assert!(physical_register_identity(&identities, 0x1000, 32).is_none());
    }
}
