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
    board::{device_label, latest},
    budget::{self, MAX_SHORT_BUDGET, format_duration},
    history::{self, LeaseOutcome, LeaseRecord},
    notify,
    process::ProcessIdentity,
    queue,
    state::{Claim, Holder, Ticket, conflict, covers, normalize},
};

/// Token of the lease a command runs inside; such commands join the lease.
pub const LEASE_ENV: &str = "OER_HIL_LEASE";
/// Who asks for the stand; defaults to the checkout directory name.
pub const OWNER_ENV: &str = "OER_HIL_OWNER";
/// Explicit budget of an implicit lease, e.g. `15m`.
pub const BUDGET_ENV: &str = "OER_HIL_BUDGET";
/// `1` marks an implicit lease as short.
pub const SHORT_ENV: &str = "OER_HIL_SHORT";

/// Time a terminated holder has to clean up before it is killed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(300);
/// How often an over-budget holder looks for requests it blocks.
const WAITER_CHECK: Duration = Duration::from_millis(500);
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
    /// What the lease is for; leases of equal work share budget estimates.
    pub work: String,
    pub budget: Option<Duration>,
    pub short: bool,
    /// HIL scenarios the lease executes, recorded for per-scenario estimates.
    pub scenarios: Vec<String>,
    /// Resources the lease needs; none claims the whole stand.
    pub claims: Vec<Claim>,
}

impl Request {
    /// A request described by the owner, budget and short-lease environment.
    pub fn from_environment(work: impl Into<String>) -> crate::Result<Self> {
        Ok(Self {
            owner: owner_from_environment(),
            work: work.into(),
            budget: std::env::var(BUDGET_ENV)
                .ok()
                .filter(|budget| !budget.is_empty())
                .map(|budget| budget::parse_duration(&budget))
                .transpose()?,
            short: std::env::var(SHORT_ENV).is_ok_and(|short| short == "1"),
            scenarios: Vec::new(),
            claims: Vec::new(),
        })
    }
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
    budget: Duration,
    ending: Arc<Ending>,
    watchdog: Option<(mpsc::Sender<()>, std::thread::JoinHandle<()>)>,
}

/// Why a lease ends early, set by its supervisor or holder.
#[derive(Default)]
struct Ending {
    /// Terminated at twice its budget.
    exceeded: AtomicBool,
    /// Terminated at its budget because it blocked waiting requests.
    preempted: AtomicBool,
    /// Asked to yield at its next boundary, for divisible work.
    yield_requested: AtomicBool,
    /// Released at a boundary to requeue its remaining work.
    yielded: AtomicBool,
}

impl Ending {
    fn outcome(&self) -> LeaseOutcome {
        if self.exceeded.load(Ordering::Relaxed) {
            LeaseOutcome::BudgetExceeded
        } else if self.preempted.load(Ordering::Relaxed) {
            LeaseOutcome::Preempted
        } else if self.yielded.load(Ordering::Relaxed) {
            LeaseOutcome::Yielded
        } else {
            LeaseOutcome::Released
        }
    }
}

enum Poll {
    Granted {
        waited: Duration,
    },
    /// A description that changes only with the position or holder, and the
    /// message including the expected start.
    Waiting {
        key: String,
        message: String,
    },
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
        let (budget, source) = budget::resolve(
            request.budget,
            &request.work,
            &request.scenarios,
            &self.history()?,
        );
        if request.short && budget > MAX_SHORT_BUDGET {
            return Err(format!(
                "a short lease must fit {}; `{}` has budget {} ({source})",
                format_duration(MAX_SHORT_BUDGET),
                request.work,
                format_duration(budget)
            )
            .into());
        }
        let id = self.transaction_after_legacy(|state| {
            let id = state.next_id;
            state.next_id += 1;
            state.queue.push(Ticket {
                id,
                owner: request.owner.clone(),
                work: request.work.clone(),
                budget_secs: budget.as_secs(),
                budget_source: source,
                short: request.short,
                process: me,
                enqueued_unix: crate::unix_now(),
                claims: claims.clone(),
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
        let waited = loop {
            match self.poll(id, &token, started)? {
                Poll::Granted { waited } => break waited,
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
            "hil-arbiter: lease #{id} granted to {} for `{}` on {}; budget {} ({source}); \
             at its budget it yields to waiting requests, at {} it is terminated",
            request.owner,
            request.work,
            describe_claims(&claims),
            format_duration(budget),
            format_duration(budget * 2)
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
                budget,
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
            if let Some(jumped) = queue::grantable(state, id) {
                let index = state
                    .queue
                    .iter()
                    .position(|ticket| ticket.id == id)
                    .ok_or("this request left the HIL queue")?;
                let ticket = state.queue.remove(index);
                state.jumped = jumped;
                state.holders.push(Holder {
                    ticket,
                    token: token.to_owned(),
                    granted_unix: crate::unix_now(),
                    over_budget: false,
                });
                return Ok(Poll::Granted {
                    waited: started.elapsed(),
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
            let key = format!(
                "waiting as #{id} for {} behind {ahead} earlier conflicting request(s); {holders}",
                describe_claims(&claims)
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

    pub fn budget(&self) -> Option<Duration> {
        self.held.as_ref().map(|held| held.budget)
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
    /// boundary: the budget is used and a waiting request needs its resources.
    pub fn yield_requested(&self) -> bool {
        self.held
            .as_ref()
            .is_some_and(|held| held.ending.yield_requested.load(Ordering::Relaxed))
    }

    /// Record that the holder releases at a boundary to requeue its work.
    pub fn mark_yielded(&self) {
        if let Some(held) = &self.held {
            held.ending.yielded.store(true, Ordering::Relaxed);
            eprintln!(
                "hil-arbiter: lease #{} of {} yields to waiting requests and queues again",
                held.id, held.owner
            );
        }
    }

    /// Record and announce that the lease has used its budget.
    pub fn warn_over_budget(&self) {
        if let Some(held) = &self.held {
            over_budget(&held.arbiter, held.id, &held.owner, &held.work, held.budget);
        }
    }

    /// Record that the holder is being terminated at twice its budget.
    pub fn mark_budget_exceeded(&self) {
        if let Some(held) = &self.held {
            held.ending.exceeded.store(true, Ordering::Relaxed);
            budget_exceeded(&held.owner, &held.work, held.budget);
        }
    }

    /// Record that the holder is being terminated at its budget because it
    /// blocks waiting requests.
    pub fn mark_preempted(&self) {
        if let Some(held) = &self.held {
            held.ending.preempted.store(true, Ordering::Relaxed);
            preempted(&held.owner, &held.work, held.budget);
        }
    }

    /// Supervise this process. At its budget the lease is reported; from then
    /// on, while a waiting request needs its resources, divisible work is
    /// asked to yield at its next boundary ([`Self::yield_requested`]) and
    /// indivisible work receives `SIGTERM` (its ordinary cancellation and
    /// cleanup). At twice the budget the process receives `SIGTERM`
    /// regardless, and `SIGKILL` after the shutdown grace.
    pub fn supervise_self(&mut self, divisible: bool) {
        let Some(held) = &mut self.held else {
            return;
        };
        let (stop, stopped) = mpsc::channel::<()>();
        let (arbiter, id, owner, work, budget) = (
            held.arbiter.clone(),
            held.id,
            held.owner.clone(),
            held.work.clone(),
            held.budget,
        );
        let ending = held.ending.clone();
        let thread = std::thread::spawn(move || {
            let wait = |duration| {
                matches!(
                    stopped.recv_timeout(duration),
                    Err(mpsc::RecvTimeoutError::Timeout)
                )
            };
            let terminate = || {
                signal_self(rustix::process::Signal::TERM);
                if wait(SHUTDOWN_GRACE) {
                    signal_self(rustix::process::Signal::KILL);
                }
            };
            if !wait(budget) {
                return;
            }
            over_budget(&arbiter, id, &owner, &work, budget);
            let ceiling = Instant::now() + budget;
            loop {
                let now = Instant::now();
                if now >= ceiling {
                    ending.exceeded.store(true, Ordering::Relaxed);
                    budget_exceeded(&owner, &work, budget);
                    return terminate();
                }
                let blocking = arbiter
                    .transaction(|state| Ok(queue::blocks_waiters(state, id)))
                    .unwrap_or(false);
                if blocking && divisible {
                    if !ending.yield_requested.swap(true, Ordering::Relaxed) {
                        eprintln!(
                            "hil-arbiter: lease #{id} of {owner} is over budget and blocks \
                             waiting requests; it yields after the current step"
                        );
                    }
                } else if blocking {
                    ending.preempted.store(true, Ordering::Relaxed);
                    preempted(&owner, &work, budget);
                    return terminate();
                }
                if !wait(WAITER_CHECK.min(ceiling - now)) {
                    return;
                }
            }
        });
        held.watchdog = Some((stop, thread));
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

fn over_budget(arbiter: &Arbiter, id: u64, owner: &str, work: &str, budget: Duration) {
    eprintln!(
        "hil-arbiter: lease #{id} of {owner} used its budget {}; it yields to waiting \
         requests and is terminated at {}",
        format_duration(budget),
        format_duration(budget * 2)
    );
    let _ = arbiter.transaction(|state| {
        if let Some(holder) = state
            .holders
            .iter_mut()
            .find(|holder| holder.ticket.id == id)
        {
            holder.over_budget = true;
        }
        Ok(())
    });
    notify::send(
        "HIL stand over budget",
        &format!("{owner}: {work} exceeded {}", format_duration(budget)),
    );
}

fn preempted(owner: &str, work: &str, budget: Duration) {
    eprintln!(
        "hil-arbiter: {owner}: `{work}` used its budget {} and blocks waiting requests; \
         terminating with cleanup",
        format_duration(budget)
    );
    notify::send(
        "HIL stand lease preempted",
        &format!(
            "{owner}: {work} used {} while others wait",
            format_duration(budget)
        ),
    );
}

fn budget_exceeded(owner: &str, work: &str, budget: Duration) {
    eprintln!(
        "hil-arbiter: {owner}: `{work}` reached twice its budget {}; terminating with cleanup",
        format_duration(budget)
    );
    notify::send(
        "HIL stand lease terminated",
        &format!("{owner}: {work} reached {}", format_duration(budget * 2)),
    );
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
            history::append(
                &history_path,
                &LeaseRecord {
                    id: held.id,
                    owner: holder.ticket.owner,
                    work: holder.ticket.work,
                    granted_unix: holder.granted_unix,
                    released_unix: crate::unix_now(),
                    budget_secs: holder.ticket.budget_secs,
                    outcome,
                    scenarios: held.scenarios.clone(),
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
