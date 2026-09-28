//! Waiting for, holding and releasing a lease.

use std::{
    io::Read as _,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use crate::{
    Arbiter,
    balance::{self, HARD_LIMIT, MIN_SLICE},
    board::{device_label, latest},
    estimate::{self, format_duration},
    history::{self, GrantReason, LeaseOutcome, LeaseRecord, OwnerBalance},
    notify,
    process::ProcessIdentity,
    queue,
    state::{Claim, Holder, Ticket, conflict, covers, normalize},
};

/// Token of the lease a command runs inside; such commands join the lease.
pub const LEASE_ENV: &str = "OER_HIL_LEASE";
/// Who asks for the stand; defaults to the checkout directory name.
pub const OWNER_ENV: &str = "OER_HIL_OWNER";
/// Variables of lease budgets, which balances replaced; setting one fails.
const RETIRED_ENV: [&str; 2] = ["OER_HIL_BUDGET", "OER_HIL_SHORT"];
/// Why budgets are refused.
pub const NO_BUDGETS: &str =
    "the stand charges the time a lease holds; there is no budget or short lease to request";

/// Time a terminated holder has to clean up before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(300);
/// How often a holder checks the hard limit and waiters that outrank it.
const SUPERVISION: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(500);
/// Waits shorter than this do not notify the user when granted.
const NOTIFY_AFTER_WAIT: Duration = Duration::from_secs(30);

/// The name of the current directory, which is the checkout for Cargo
/// commands run from a repository root.
pub fn default_owner() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|directory| {
            directory
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| String::from("unknown"))
}

pub(crate) fn owner_from_environment() -> String {
    std::env::var(OWNER_ENV)
        .ok()
        .filter(|owner| !owner.trim().is_empty())
        .unwrap_or_else(default_owner)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    pub owner: String,
    /// What the lease is for; leases of equal work share duration estimates.
    pub work: String,
    /// HIL scenarios the lease executes, recorded for per-scenario estimates.
    pub scenarios: Vec<String>,
    /// Resources the lease needs; none claims the whole stand.
    pub claims: Vec<Claim>,
}

impl Request {
    /// A request by the owner the environment names.
    pub fn from_environment(work: impl Into<String>) -> crate::Result<Self> {
        if let Some(variable) = retired_variable(|name| std::env::var_os(name)) {
            return Err(format!("{variable} is set, but {NO_BUDGETS}").into());
        }
        Ok(Self {
            owner: owner_from_environment(),
            work: work.into(),
            scenarios: Vec::new(),
            claims: Vec::new(),
        })
    }
}

/// The retired budget variable `lookup` finds set, if any.
fn retired_variable(lookup: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<&'static str> {
    RETIRED_ENV
        .into_iter()
        .find(|variable| lookup(variable).is_some_and(|value| !value.is_empty()))
}

/// Ownership of the stand. A nested grant belongs to an enclosing lease and
/// releases nothing.
pub struct Grant {
    held: Option<Held>,
}

struct Held {
    arbiter: Arbiter,
    id: u64,
    token: String,
    owner: String,
    work: String,
    scenarios: Vec<String>,
    held_since: Instant,
    ending: Arc<Ending>,
    watchdog: Option<(mpsc::Sender<()>, std::thread::JoinHandle<()>)>,
}

/// Why a lease ends early, set by its supervisor or holder.
#[derive(Default)]
struct Ending {
    /// Terminated at the hard limit.
    hard_limit: AtomicBool,
    /// Asked to yield at its next boundary, for divisible work.
    yield_requested: AtomicBool,
    /// Released at a boundary to requeue its remaining work.
    yielded: AtomicBool,
}

impl Ending {
    fn outcome(&self) -> LeaseOutcome {
        if self.hard_limit.load(Ordering::Relaxed) {
            LeaseOutcome::HardLimit
        } else if self.yielded.load(Ordering::Relaxed) {
            LeaseOutcome::YieldedToBalance
        } else {
            LeaseOutcome::Released
        }
    }
}

enum Poll {
    Granted {
        waited: Duration,
        /// The owner's balance at the grant, in milliseconds.
        balance: i64,
    },
    /// A description that changes only with the position or holder, and the
    /// message including the expected start.
    Waiting { key: String, message: String },
}

/// A waiting message is repeated at this interval when nothing changed.
const REPORT_INTERVAL: Duration = Duration::from_secs(60);

impl Arbiter {
    /// Wait in the queue until the stand is granted to `request`.
    ///
    /// A process inside a lease (its token in [`LEASE_ENV`]) or already
    /// holding one joins it without waiting. Cancellation leaves the queue.
    pub fn acquire(&self, request: &Request) -> crate::Result<Grant> {
        let enclosing = std::env::var(LEASE_ENV)
            .ok()
            .filter(|token| !token.is_empty());
        self.acquire_within(request, enclosing)
    }

    pub(crate) fn acquire_within(
        &self,
        request: &Request,
        enclosing: Option<String>,
    ) -> crate::Result<Grant> {
        let me = ProcessIdentity::current()?;
        let claims = normalize(&request.claims);
        if let Some(nested) = self.join_enclosing(me, enclosing, &claims)? {
            return Ok(nested);
        }
        if let Some(refusal) =
            crate::maintenance::refusal(&self.maintenance()?, &request.owner, &claims)
        {
            return Err(refusal.into());
        }
        let (estimate, _) = estimate::estimate(&request.work, &request.scenarios, &self.history()?);
        let id = self.transaction_after_legacy(|state| {
            let id = state.next_id;
            state.next_id += 1;
            state.queue.push(Ticket {
                id,
                owner: request.owner.clone(),
                work: request.work.clone(),
                estimate_secs: estimate.as_secs(),
                process: me,
                enqueued_unix: crate::unix_now(),
                claims: claims.clone(),
                unknown: Default::default(),
            });
            Ok(id)
        })?;
        let mut waiting = Waiting {
            arbiter: self,
            id,
            granted: false,
        };
        let token = token()?;
        let started = Instant::now();
        let mut reported: Option<(String, Instant)> = None;
        let (waited, balance) = loop {
            match self.poll(id, &token, started)? {
                Poll::Granted { waited, balance } => break (waited, balance),
                Poll::Waiting { key, message } => {
                    if reported.as_ref().is_none_or(|(previous, at)| {
                        *previous != key || at.elapsed() >= REPORT_INTERVAL
                    }) {
                        eprintln!("hil-arbiter: {message}");
                        reported = Some((key, Instant::now()));
                    }
                }
            }
            oer_process::sleep(POLL)?;
        };
        waiting.granted = true;
        eprintln!(
            "hil-arbiter: lease #{id} granted to {} for `{}` on {}; balance {}; the time it \
             holds is charged, divisible work yields after {} to a waiter with a higher \
             balance, and every lease ends at {}",
            request.owner,
            request.work,
            describe_claims(&claims),
            signed_duration(balance),
            format_duration(MIN_SLICE),
            format_duration(HARD_LIMIT)
        );
        if waited >= NOTIFY_AFTER_WAIT {
            notify::send(
                "HIL stand granted",
                &format!(
                    "{} got the stand after {} for {}",
                    request.owner,
                    format_duration(waited),
                    request.work
                ),
            );
        }
        self.report_board(&request.owner);
        Ok(Grant {
            held: Some(Held {
                arbiter: self.clone(),
                id,
                token,
                owner: request.owner.clone(),
                work: request.work.clone(),
                scenarios: request.scenarios.clone(),
                held_since: Instant::now(),
                ending: Arc::new(Ending::default()),
                watchdog: None,
            }),
        })
    }

    fn join_enclosing(
        &self,
        me: ProcessIdentity,
        token: Option<String>,
        claims: &[Claim],
    ) -> crate::Result<Option<Grant>> {
        let enclosing = self.transaction_after_legacy(|state| {
            Ok(state
                .holders
                .iter()
                .find(|holder| {
                    token.as_deref() == Some(holder.token.as_str()) || holder.ticket.process == me
                })
                .map(|holder| holder.ticket.claims.clone()))
        })?;
        match (enclosing, token) {
            (Some(held), _) if covers(&held, claims) => Ok(Some(Grant { held: None })),
            (Some(held), _) => Err(format!(
                "the enclosing lease holds {} but this command needs {}; claim it in the \
                 enclosing lease",
                describe_claims(&held),
                describe_claims(claims)
            )
            .into()),
            (None, Some(_)) => Err(format!(
                "{LEASE_ENV} names a lease that is no longer held; the enclosing lease ended"
            )
            .into()),
            (None, None) => Ok(None),
        }
    }

    fn poll(&self, id: u64, token: &str, started: Instant) -> crate::Result<Poll> {
        self.transaction(|state| {
            if queue::grantable(state, id) {
                let index = state
                    .queue
                    .iter()
                    .position(|ticket| ticket.id == id)
                    .ok_or("this request left the HIL queue")?;
                let ticket = state.queue.remove(index);
                let balance = balance::of(&state.balances, &ticket.owner);
                let over = state
                    .queue
                    .iter()
                    .filter(|other| {
                        other.owner != ticket.owner && conflict(&other.claims, &ticket.claims)
                    })
                    .map(|other| OwnerBalance {
                        owner: other.owner.clone(),
                        balance_ms: balance::of(&state.balances, &other.owner),
                    })
                    .collect();
                state.holders.push(Holder {
                    ticket,
                    token: token.to_owned(),
                    granted_unix: crate::unix_now(),
                    reason: Some(GrantReason {
                        balance_ms: balance,
                        over,
                    }),
                    preempted: None,
                    unknown: Default::default(),
                });
                balance::normalize(state, crate::unix_now_ms());
                return Ok(Poll::Granted {
                    waited: started.elapsed(),
                    balance,
                });
            }
            let (_, ahead, wait) = queue::expected_starts(state, crate::unix_now())
                .into_iter()
                .find(|(ticket, ..)| *ticket == id)
                .ok_or("this request left the HIL queue")?;
            let claims = state
                .queue
                .iter()
                .find(|ticket| ticket.id == id)
                .map(|ticket| ticket.claims.clone())
                .unwrap_or_default();
            let blocking = state
                .holders
                .iter()
                .filter(|holder| conflict(&holder.ticket.claims, &claims))
                .map(|holder| format!("{} `{}`", holder.ticket.owner, holder.ticket.work))
                .collect::<Vec<_>>();
            let holders = if blocking.is_empty() {
                String::from("no conflicting holder")
            } else {
                format!("held by {}", blocking.join(", "))
            };
            let owner = state
                .queue
                .iter()
                .find(|ticket| ticket.id == id)
                .map(|ticket| ticket.owner.clone())
                .unwrap_or_default();
            let key = format!(
                "waiting as #{id} for {} with balance {}, behind {ahead} conflicting request(s) \
                 of owners with a higher balance; {holders}",
                describe_claims(&claims),
                signed_duration(balance::of(&state.balances, &owner))
            );
            Ok(Poll::Waiting {
                message: format!(
                    "{key}; expected start in ~{}",
                    format_duration(Duration::from_secs(wait))
                ),
                key,
            })
        })
    }

    /// Show every board's newest firmware and the flashes other owners made
    /// since this owner's previous lease.
    fn report_board(&self, owner: &str) {
        let (Ok(history), Ok(events)) = (self.history(), self.board_events()) else {
            eprintln!("hil-arbiter: board journal unavailable");
            return;
        };
        let devices = self.devices().unwrap_or_default();
        let label = |event: &crate::BoardEvent| device_label(event.device.as_deref(), &devices);
        let previous = history
            .iter()
            .rev()
            .find(|record| record.owner == owner)
            .map(|record| record.released_unix);
        if let Some(previous) = previous {
            let changes = events
                .iter()
                .filter(|event| {
                    event.owner != owner
                        && event.unix >= previous
                        && matches!(event.kind, crate::BoardEventKind::Flashed { .. })
                })
                .collect::<Vec<_>>();
            if !changes.is_empty() {
                eprintln!("hil-arbiter: flashes by others since your previous lease:");
                for event in changes.iter().rev().take(10).rev() {
                    eprintln!("hil-arbiter:   {}: {event}", label(event));
                }
            }
        }
        let (flashes, _) = latest(&events);
        if flashes.is_empty() {
            eprintln!("hil-arbiter: board firmware: unknown");
        }
        for flash in flashes {
            eprintln!("hil-arbiter: firmware on {}: {flash}", label(flash));
        }
    }
}

/// Removes the ticket of a request that stops waiting.
struct Waiting<'a> {
    arbiter: &'a Arbiter,
    id: u64,
    granted: bool,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if !self.granted {
            let id = self.id;
            let _ = self.arbiter.transaction(|state| {
                state.queue.retain(|ticket| ticket.id != id);
                Ok(())
            });
        }
    }
}

impl Grant {
    /// Whether this grant joined an enclosing lease.
    pub fn is_nested(&self) -> bool {
        self.held.is_none()
    }

    /// Variables that make commands started inside the lease join it.
    pub fn environment(&self) -> Vec<(&'static str, String)> {
        self.held.as_ref().map_or_else(Vec::new, |held| {
            vec![
                (LEASE_ENV, held.token.clone()),
                (OWNER_ENV, held.owner.clone()),
            ]
        })
    }

    /// Whether a waiting request needs a resource this lease holds.
    pub fn blocks_waiters(&self) -> bool {
        self.held.as_ref().is_some_and(|held| {
            held.arbiter
                .transaction(|state| Ok(queue::blocks_waiters(state, held.id)))
                .unwrap_or(false)
        })
    }

    /// Whether the supervisor asked divisible work to yield at its next
    /// boundary: a waiter it blocks has a higher balance and the lease has
    /// held its minimum slice.
    pub fn yield_requested(&self) -> bool {
        self.held
            .as_ref()
            .is_some_and(|held| held.ending.yield_requested.load(Ordering::Relaxed))
    }

    /// Record that the holder is being terminated at the hard limit.
    pub fn mark_hard_limit(&self) {
        if let Some(held) = &self.held {
            held.ending.hard_limit.store(true, Ordering::Relaxed);
            eprintln!(
                "hil-arbiter: {}: `{}` reached the hard limit {}; terminating",
                held.owner,
                held.work,
                format_duration(HARD_LIMIT)
            );
        }
    }

    /// Record that the holder releases at a boundary to requeue its work.
    pub fn mark_yielded(&self) {
        if let Some(held) = &self.held {
            held.ending.yielded.store(true, Ordering::Relaxed);
            eprintln!(
                "hil-arbiter: lease #{} of {} yields to a waiter with a higher balance and \
                 queues again",
                held.id, held.owner
            );
        }
    }

    /// Supervise this process. Divisible work is asked to yield at its next
    /// boundary ([`Self::yield_requested`]) once it has held
    /// [`MIN_SLICE`] and a waiter it blocks has a higher balance; indivisible
    /// work runs to completion. At [`HARD_LIMIT`] the process receives
    /// `SIGTERM`, its ordinary cancellation and cleanup, and `SIGKILL` after
    /// the shutdown grace.
    pub fn supervise_self(&mut self, divisible: bool) {
        self.supervise_with(divisible, MIN_SLICE, HARD_LIMIT);
    }

    /// [`Self::supervise_self`] with explicit limits, for tests.
    pub(crate) fn supervise_with(&mut self, divisible: bool, slice: Duration, limit: Duration) {
        let Some(held) = &mut self.held else {
            return;
        };
        let (stop, stopped) = mpsc::channel::<()>();
        let (arbiter, id, owner, work, since) = (
            held.arbiter.clone(),
            held.id,
            held.owner.clone(),
            held.work.clone(),
            held.held_since,
        );
        let ending = held.ending.clone();
        let thread = std::thread::spawn(move || {
            let wait = |duration| {
                matches!(
                    stopped.recv_timeout(duration),
                    Err(mpsc::RecvTimeoutError::Timeout)
                )
            };
            loop {
                if since.elapsed() >= limit {
                    ending.hard_limit.store(true, Ordering::Relaxed);
                    eprintln!(
                        "hil-arbiter: {owner}: `{work}` reached the hard limit {}; terminating \
                         with cleanup",
                        format_duration(limit)
                    );
                    notify::send(
                        "HIL stand lease terminated",
                        &format!("{owner}: {work} reached {}", format_duration(limit)),
                    );
                    signal_self(rustix::process::Signal::TERM);
                    if wait(SHUTDOWN_GRACE) {
                        signal_self(rustix::process::Signal::KILL);
                    }
                    return;
                }
                if divisible
                    && !ending.yield_requested.load(Ordering::Relaxed)
                    && let Ok(Some((waiter, balance))) = arbiter.transaction(|state| {
                        Ok(queue::outranked_by(state, id, crate::unix_now(), slice))
                    })
                {
                    ending.yield_requested.store(true, Ordering::Relaxed);
                    eprintln!(
                        "hil-arbiter: lease #{id} of {owner} yields after the current step to \
                         {waiter}, whose balance {} is higher",
                        signed_duration(balance)
                    );
                }
                if !wait(SUPERVISION) {
                    return;
                }
            }
        });
        held.watchdog = Some((stop, thread));
    }
}

/// `+12m`, `-3m30s`, `0s`.
pub(crate) fn signed_duration(milliseconds: i64) -> String {
    let magnitude = format_duration(Duration::from_millis(milliseconds.unsigned_abs()));
    match milliseconds.signum() {
        1 => format!("+{magnitude}"),
        -1 => format!("-{magnitude}"),
        _ => magnitude,
    }
}

/// `board:AA, air (shared)` or `the whole stand`.
pub(crate) fn describe_claims(claims: &[Claim]) -> String {
    if claims.iter().any(|claim| claim.resource == crate::STAND) {
        return String::from("the whole stand");
    }
    claims
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn signal_self(signal: rustix::process::Signal) {
    let _ = rustix::process::kill_process(rustix::process::getpid(), signal);
}

impl Drop for Grant {
    fn drop(&mut self) {
        let Some(mut held) = self.held.take() else {
            return;
        };
        if let Some((stop, thread)) = held.watchdog.take() {
            drop(stop);
            let _ = thread.join();
        }
        let history_path = held.arbiter.history_path();
        let outcome = held.ending.outcome();
        let released = held.arbiter.transaction(|state| {
            let Some(index) = state
                .holders
                .iter()
                .position(|holder| holder.ticket.id == held.id)
            else {
                return Ok(None);
            };
            let holder = state.holders.remove(index);
            if let Some(preemption) = &holder.preempted {
                eprintln!(
                    "hil-arbiter: lease #{} was preempted by {}: {}",
                    held.id, preemption.by, preemption.reason
                );
            }
            let charged_ms =
                crate::preempt::charged_ms(&holder, held.held_since.elapsed().as_millis() as u64);
            history::append(
                &history_path,
                &LeaseRecord {
                    id: held.id,
                    balance_after_ms: balance::of(&state.balances, &holder.ticket.owner),
                    owner: holder.ticket.owner,
                    work: holder.ticket.work,
                    granted_unix: holder.granted_unix,
                    released_unix: crate::unix_now(),
                    outcome: if holder.preempted.is_some() {
                        LeaseOutcome::PreemptedOnRequest
                    } else {
                        outcome
                    },
                    charged_ms,
                    reason: holder.reason,
                    preempted: holder.preempted,
                    scenarios: held.scenarios.clone(),
                    unknown: Default::default(),
                },
            )?;
            Ok(Some(state.queue.is_empty() && state.holders.is_empty()))
        });
        match released {
            Ok(Some(idle)) => {
                eprintln!("hil-arbiter: lease #{} released", held.id);
                if idle {
                    notify::send(
                        "HIL stand free",
                        &format!("{} finished {}", held.owner, held.work),
                    );
                }
            }
            Ok(None) => {}
            Err(error) => eprintln!("hil-arbiter: cannot release lease #{}: {error}", held.id),
        }
    }
}

fn token() -> crate::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests;
