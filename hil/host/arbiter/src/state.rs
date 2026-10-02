//! The shared queue, the lease holders and the resources they claim.

use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use crate::{balance::Balance, process::ProcessIdentity};

/// Schema 3 serves requests by owner balance instead of arrival and budget.
pub(crate) const STATE_SCHEMA: u32 = 3;

/// Claims everything: a lease that names no resources.
pub const STAND: &str = "stand";
/// The shared radio environment. RF-sensitive work claims it exclusively;
/// other radio work claims it shared.
pub const AIR: &str = "air";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Exclusive,
    Shared,
}

/// One resource a lease holds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub resource: String,
    pub mode: Mode,
}

impl Claim {
    pub fn exclusive(resource: impl Into<String>) -> Self {
        Self {
            resource: resource.into(),
            mode: Mode::Exclusive,
        }
    }

    pub fn shared(resource: impl Into<String>) -> Self {
        Self {
            resource: resource.into(),
            mode: Mode::Shared,
        }
    }

    /// A board, identified by its MAC or, without one, its port.
    pub fn board(identity: &str) -> Self {
        Self::exclusive(format!("board:{identity}"))
    }

    pub fn stand() -> Self {
        Self::exclusive(STAND)
    }
}

impl std::fmt::Display for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(range) = crate::spectrum::Spectrum::parse(&self.resource) {
            return range.fmt(f);
        }
        match self.mode {
            Mode::Exclusive => f.write_str(&self.resource),
            Mode::Shared => write!(f, "{} (shared)", self.resource),
        }
    }
}

/// Claims without duplicates, an exclusive claim absorbing a shared one;
/// no claims means the whole stand.
pub(crate) fn normalize(claims: &[Claim]) -> Vec<Claim> {
    if claims.is_empty() || claims.iter().any(|claim| claim.resource == STAND) {
        return vec![Claim::stand()];
    }
    let mut normalized: Vec<Claim> = Vec::new();
    for claim in claims {
        match normalized
            .iter_mut()
            .find(|known| known.resource == claim.resource)
        {
            Some(known) if claim.mode == Mode::Exclusive => known.mode = Mode::Exclusive,
            Some(_) => {}
            None => normalized.push(claim.clone()),
        }
    }
    normalized.sort_by(|a, b| a.resource.cmp(&b.resource));
    normalized
}

/// Whether two sets of claims cannot be held at once. When both carry
/// frequency ranges, the ranges order their radio use and the coarse air
/// claim is left to arbiters without ranges.
pub(crate) fn conflict(a: &[Claim], b: &[Claim]) -> bool {
    use crate::spectrum::{is_range, ranges};
    let whole = |claims: &[Claim]| claims.iter().any(|claim| claim.resource == STAND);
    if whole(a) || whole(b) {
        return true;
    }
    let (a_ranges, b_ranges) = (ranges(a), ranges(b));
    let by_ranges = !a_ranges.is_empty() && !b_ranges.is_empty();
    if by_ranges
        && a_ranges
            .iter()
            .any(|x| b_ranges.iter().any(|y| x.conflicts(y)))
    {
        return true;
    }
    a.iter().any(|x| {
        b.iter().any(|y| {
            x.resource == y.resource
                && !is_range(&x.resource)
                && !(by_ranges && x.resource == AIR)
                && (x.mode == Mode::Exclusive || y.mode == Mode::Exclusive)
        })
    })
}

/// Whether a lease holding `outer` already holds everything in `inner`.
pub(crate) fn covers(outer: &[Claim], inner: &[Claim]) -> bool {
    use crate::spectrum::Spectrum;
    let held_ranges = crate::spectrum::ranges(outer);
    outer.iter().any(|claim| claim.resource == STAND)
        || inner.iter().all(|wanted| {
            if let Some(range) = Spectrum::parse(&wanted.resource) {
                // An exclusive coarse air claim holds every range.
                return held_ranges.iter().any(|held| held.covers(&range))
                    || outer
                        .iter()
                        .any(|held| held.resource == AIR && held.mode == Mode::Exclusive);
            }
            wanted.resource != STAND
                && outer.iter().any(|held| {
                    held.resource == wanted.resource
                        && (held.mode == Mode::Exclusive || wanted.mode == Mode::Shared)
                })
        })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct State {
    pub(crate) schema: u32,
    pub(crate) next_id: u64,
    pub(crate) queue: Vec<Ticket>,
    pub(crate) holders: Vec<Holder>,
    /// Every recently active owner's balance; see [`crate::balance`].
    #[serde(default)]
    pub(crate) balances: BTreeMap<String, Balance>,
    /// When the balances were last advanced; 0 before the first settlement.
    #[serde(default)]
    pub(crate) settled_unix_ms: u64,
    /// Fields a newer build wrote, kept when this build rewrites the record.
    #[serde(flatten)]
    pub(crate) unknown: crate::Unknown,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema: STATE_SCHEMA,
            next_id: 1,
            queue: Vec::new(),
            holders: Vec::new(),
            balances: BTreeMap::new(),
            settled_unix_ms: 0,
            unknown: crate::Unknown::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Ticket {
    pub(crate) id: u64,
    pub(crate) owner: String,
    pub(crate) work: String,
    /// Expected duration from earlier leases, for waiting-time estimates
    /// only; never a limit.
    #[serde(default)]
    pub(crate) estimate_secs: u64,
    pub(crate) process: ProcessIdentity,
    pub(crate) enqueued_unix: u64,
    pub(crate) claims: Vec<Claim>,
    /// Maintenance goes before every ordinary request.
    #[serde(default, skip_serializing_if = "Priority::is_ordinary")]
    pub(crate) priority: Priority,
    /// Fields a newer build wrote, kept when this build rewrites the record.
    #[serde(flatten)]
    pub(crate) unknown: crate::Unknown,
}

/// How a request is ordered against the others.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Priority {
    /// Served by its owner's balance.
    #[default]
    Ordinary,
    /// Stand maintenance, such as a fixture software installation: served
    /// before every ordinary request, and it preempts the holders it
    /// conflicts with.
    Maintenance,
}

impl Priority {
    fn is_ordinary(&self) -> bool {
        *self == Self::Ordinary
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Holder {
    pub(crate) ticket: Ticket,
    /// Exported to commands inside the lease, which then join it.
    pub(crate) token: String,
    pub(crate) granted_unix: u64,
    /// The balances the grant was decided by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<crate::history::GrantReason>,
    /// Set once `cargo hil preempt` stopped the lease; it is charged no
    /// longer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) preempted: Option<crate::preempt::Preemption>,
    /// Fields a newer build wrote, kept when this build rewrites the record.
    #[serde(flatten)]
    pub(crate) unknown: crate::Unknown,
}

/// Why a state file of another schema cannot be read. The arbiter reads one
/// schema: a newer state needs a newer checkout, and an older one is never
/// converted, so the operator drains or resets it.
pub(crate) fn unreadable(path: &std::path::Path, schema: &serde_json::Value) -> String {
    let path = path.display();
    match schema.as_u64() {
        Some(schema) if schema > u64::from(STATE_SCHEMA) => format!(
            "HIL arbiter state {path} has schema {schema}; this checkout reads schema \
             {STATE_SCHEMA}. Update the checkout"
        ),
        _ => format!(
            "HIL arbiter state {path} has schema {schema}; this checkout reads only schema \
             {STATE_SCHEMA} and does not convert older states. Let the checkouts that hold or \
             wait for the stand finish, then remove {path} to reset the queue"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(list: &[(&str, Mode)]) -> Vec<Claim> {
        list.iter()
            .map(|(resource, mode)| Claim {
                resource: (*resource).into(),
                mode: *mode,
            })
            .collect()
    }

    #[test]
    fn claims_conflict_on_exclusive_use_and_on_the_whole_stand() {
        use Mode::{Exclusive, Shared};
        let s31 = claims(&[("board:esp32s31", Exclusive), (AIR, Shared)]);
        let c5 = claims(&[("board:esp32c5", Exclusive), (AIR, Shared)]);
        let rf = claims(&[("board:esp32c5", Exclusive), (AIR, Exclusive)]);
        assert!(!conflict(&s31, &c5), "different boards share the air");
        assert!(
            conflict(&s31, &rf),
            "exclusive air excludes other radio work"
        );
        assert!(conflict(&s31, &s31), "one board has one holder");
        assert!(conflict(&[Claim::stand()], &c5));
    }

    #[test]
    fn ranges_order_radio_use_between_new_leases_and_the_air_claim_for_old_ones() {
        use crate::spectrum::{BAND_2G4, BAND_5G, Emits, Need, Spectrum};
        use Mode::{Exclusive, Shared};
        let with = |board: &str, range: Spectrum| {
            let mut claims = vec![Claim::board(board)];
            claims.extend(range.claims());
            normalize(&claims)
        };
        let calibration = with("a", Spectrum::new(BAND_2G4, Need::Strict, Emits::Normal));
        let five = with("b", Spectrum::new(BAND_5G, Need::Tolerant, Emits::Normal));
        let peer = with("d", Spectrum::ieee802154(15, Need::Tolerant, Emits::Normal));
        assert!(
            !conflict(&calibration, &five),
            "disjoint bands run together"
        );
        assert!(
            conflict(&calibration, &peer),
            "a strict band excludes a transmitter in it"
        );
        assert!(!conflict(&five, &peer));
        // A lease from a checkout without ranges claims only the air.
        let old_shared = claims(&[("board:c", Exclusive), (AIR, Shared)]);
        let old_exclusive = claims(&[("board:c", Exclusive), (AIR, Exclusive)]);
        assert!(
            conflict(&calibration, &old_shared),
            "strict holds the air exclusively"
        );
        assert!(!conflict(&five, &old_shared));
        assert!(conflict(&five, &old_exclusive));
        assert!(!conflict(
            &claims(&[("x", Shared)]),
            &claims(&[("y", Exclusive)])
        ));
    }

    #[test]
    fn normalization_merges_modes_and_defaults_to_the_stand() {
        use Mode::{Exclusive, Shared};
        assert_eq!(normalize(&[]), [Claim::stand()]);
        assert_eq!(
            normalize(&claims(&[
                (AIR, Shared),
                ("board:a", Exclusive),
                (AIR, Exclusive)
            ])),
            claims(&[(AIR, Exclusive), ("board:a", Exclusive)])
        );
    }

    #[test]
    fn a_lease_covers_what_it_holds_at_least_as_strongly() {
        use Mode::{Exclusive, Shared};
        let held = claims(&[("board:a", Exclusive), (AIR, Shared)]);
        assert!(covers(&held, &claims(&[("board:a", Exclusive)])));
        assert!(covers(&held, &claims(&[(AIR, Shared)])));
        assert!(!covers(&held, &claims(&[(AIR, Exclusive)])));
        assert!(!covers(&held, &[Claim::stand()]));
        assert!(covers(&[Claim::stand()], &held));
    }

    #[test]
    fn a_state_of_another_schema_names_its_remedy() {
        let path = std::path::Path::new("/stand/state.json");
        let older = unreadable(path, &serde_json::json!(1));
        assert!(older.contains("does not convert"), "{older}");
        assert!(older.contains("remove /stand/state.json"), "{older}");
        assert!(unreadable(path, &serde_json::Value::Null).contains("does not convert"));
        let newer = unreadable(path, &serde_json::json!(STATE_SCHEMA + 1));
        assert!(newer.contains("Update the checkout"), "{newer}");
    }
}
