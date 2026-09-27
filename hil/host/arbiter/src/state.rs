//! The shared queue, the lease holders and the resources they claim.

use serde::{Deserialize, Serialize};

use crate::{budget::BudgetSource, process::ProcessIdentity};

pub(crate) const STATE_SCHEMA: u32 = 2;

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

/// Whether two sets of claims cannot be held at once.
pub(crate) fn conflict(a: &[Claim], b: &[Claim]) -> bool {
    let whole = |claims: &[Claim]| claims.iter().any(|claim| claim.resource == STAND);
    whole(a)
        || whole(b)
        || a.iter().any(|x| {
            b.iter().any(|y| {
                x.resource == y.resource && (x.mode == Mode::Exclusive || y.mode == Mode::Exclusive)
            })
        })
}

/// Whether a lease holding `outer` already holds everything in `inner`.
pub(crate) fn covers(outer: &[Claim], inner: &[Claim]) -> bool {
    outer.iter().any(|claim| claim.resource == STAND)
        || inner.iter().all(|wanted| {
            wanted.resource != STAND
                && outer.iter().any(|held| {
                    held.resource == wanted.resource
                        && (held.mode == Mode::Exclusive || wanted.mode == Mode::Shared)
                })
        })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct State {
    pub(crate) schema: u32,
    pub(crate) next_id: u64,
    pub(crate) queue: Vec<Ticket>,
    pub(crate) holders: Vec<Holder>,
    /// The previous grant went to a short request ahead of earlier
    /// conflicting requests, so the next grant may not jump.
    pub(crate) jumped: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema: STATE_SCHEMA,
            next_id: 1,
            queue: Vec::new(),
            holders: Vec::new(),
            jumped: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Ticket {
    pub(crate) id: u64,
    pub(crate) owner: String,
    pub(crate) work: String,
    pub(crate) budget_secs: u64,
    pub(crate) budget_source: BudgetSource,
    pub(crate) short: bool,
    pub(crate) process: ProcessIdentity,
    pub(crate) enqueued_unix: u64,
    pub(crate) claims: Vec<Claim>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Holder {
    pub(crate) ticket: Ticket,
    /// Exported to commands inside the lease, which then join it.
    pub(crate) token: String,
    pub(crate) granted_unix: u64,
    pub(crate) over_budget: bool,
}

/// Whether a schema 1 state has a live holder or waiting ticket.
pub(crate) fn legacy_live(value: &serde_json::Value) -> bool {
    let live = |ticket: &serde_json::Value| {
        serde_json::from_value::<ProcessIdentity>(ticket["process"].clone())
            .is_ok_and(|process| process.alive())
    };
    live(&value["holder"]["ticket"])
        || value["queue"]
            .as_array()
            .is_some_and(|queue| queue.iter().any(live))
}

/// Read schema 1, one whole-stand holder, as schema 2 with that holder and
/// every ticket claiming the whole stand.
pub(crate) fn migrate(mut value: serde_json::Value) -> crate::Result<State> {
    if value["schema"] == 1 {
        let whole = serde_json::to_value(vec![Claim::stand()])?;
        let add_claims = |ticket: &mut serde_json::Value| {
            ticket["claims"] = whole.clone();
        };
        let object = value
            .as_object_mut()
            .ok_or("arbiter state is not an object")?;
        if let Some(queue) = object
            .get_mut("queue")
            .and_then(|queue| queue.as_array_mut())
        {
            queue.iter_mut().for_each(add_claims);
        }
        let holders = match object.remove("holder") {
            Some(serde_json::Value::Null) | None => Vec::new(),
            Some(mut holder) => {
                add_claims(&mut holder["ticket"]);
                vec![holder]
            }
        };
        object.insert("holders".into(), serde_json::Value::Array(holders));
        let jumped = object
            .remove("head_next")
            .unwrap_or(serde_json::Value::Bool(false));
        object.insert("jumped".into(), jumped);
        object.insert("schema".into(), STATE_SCHEMA.into());
    }
    Ok(serde_json::from_value(value)?)
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
        let s31 = claims(&[("board:s31", Exclusive), (AIR, Shared)]);
        let c5 = claims(&[("board:c5", Exclusive), (AIR, Shared)]);
        let rf = claims(&[("board:c5", Exclusive), (AIR, Exclusive)]);
        assert!(!conflict(&s31, &c5), "different boards share the air");
        assert!(
            conflict(&s31, &rf),
            "exclusive air excludes other radio work"
        );
        assert!(conflict(&s31, &s31), "one board has one holder");
        assert!(conflict(&[Claim::stand()], &c5));
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
    fn schema_one_state_becomes_whole_stand_claims() {
        let ticket = serde_json::json!({
            "id": 2, "owner": "phy", "work": "run", "budget_secs": 60,
            "budget_source": {"kind": "explicit"}, "short": false,
            "process": {"pid": 1, "start_ticks": 1}, "enqueued_unix": 5
        });
        let state = migrate(serde_json::json!({
            "schema": 1, "next_id": 3, "queue": [ticket.clone()],
            "holder": {"ticket": ticket, "token": "t", "granted_unix": 6, "over_budget": false},
            "head_next": true
        }))
        .unwrap();
        assert_eq!(state.schema, STATE_SCHEMA);
        assert_eq!(state.holders.len(), 1);
        assert_eq!(state.queue[0].claims, [Claim::stand()]);
        assert_eq!(state.holders[0].ticket.claims, [Claim::stand()]);
        assert!(state.jumped);
    }

    #[test]
    fn a_schema_one_state_is_live_while_its_processes_run() {
        let me = serde_json::to_value(ProcessIdentity::current().unwrap()).unwrap();
        let gone = serde_json::json!({"pid": u32::MAX, "start_ticks": 1});
        let state = |holder: serde_json::Value, queued: serde_json::Value| serde_json::json!({"schema": 1, "holder": holder, "queue": [{"process": queued}]});
        assert!(legacy_live(&state(
            serde_json::json!({"ticket": {"process": me}}),
            gone.clone()
        )));
        assert!(legacy_live(&state(serde_json::Value::Null, me)));
        assert!(!legacy_live(&state(serde_json::Value::Null, gone)));
    }
}
