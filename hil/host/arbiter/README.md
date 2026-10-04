# HIL stand arbiter

`oer-hil-arbiter` orders access to the HIL stand between every checkout of one
host user. The [host guide](../stand.md) describes the
commands; this crate owns the queue, leases, claims, owner balances and the board
journal.

## Ownership

- Every lease is charged to one owner from a closed set of agents
  (`Owner`). A checkout registers its owner once with `cargo hil owner set
  NAME`; a request from an unregistered checkout without an explicit owner is
  refused. `Arbiter::merge_owner` moves an old owner name's balance and
  history to a registered one.
- A request claims resources, each exclusively or shared: `board:<MAC>`,
  fixture keys, `air`, or `stand`, which conflicts with everything and is the
  claim of a request that names nothing. Holders whose claims do not conflict
  hold leases at once.
- `Arbiter::acquire` enqueues a ticket and waits until no holder and no
  waiting ticket served before it conflicts with it: a maintenance ticket
  first, then the ticket whose owner has the higher balance, the earlier one
  on a tie (`balance`). `Arbiter::acquire_maintenance` enqueues a
  `Priority::Maintenance` ticket, used by fixture software installation,
  and preempts every conflicting holder as `preempt` does. A ticket's
  expected start counts the expected starts of the conflicting tickets
  served before it, and a holder a maintenance ticket preempts ends within
  the shutdown grace. Every
  transaction first advances the balances to now: they decay with a two-hour
  half-life, every held lease charges its owner the elapsed time, an owner
  with a ticket blocked by another owner's holder or by another owner's
  ticket served before it is credited it once (a ticket behind its own
  owner's lease earns nothing), the mean over the owners active in the last
  day is subtracted, and they are clamped to an hour either way.
  A process that already holds a lease, or carries its token in
  `OER_HIL_LEASE`, joins it without waiting when the lease covers its claims;
  a token of an ended lease or a claim outside the lease is an error.
- `Grant` releases the lease on drop and records it in the history with its
  outcome, the time charged, the owner's balance and the balances the grant
  was decided by. `supervise_self` makes the holder its own watchdog:
  divisible work that has held `MIN_SLICE` (ten minutes) sees
  `yield_requested` when a waiting ticket it blocks belongs to an owner with a
  higher balance, and yields at its next boundary; indivisible work runs to
  its end. At `HARD_LIMIT` (one hour) the holder receives `SIGTERM`, and
  `SIGKILL` after the shutdown grace. `cargo hil lease` stops its command at
  the same limit. Nothing is requested in advance; the retired budget options
  and variables are refused.
- `Arbiter::preempt` (`cargo hil preempt ID --reason TEXT`) marks a holder
  preempted, which stops its charge in the next settlement, then signals its
  verified process `SIGTERM` and `SIGKILL` after the grace. The holder's
  release, or the reaping of a killed holder, records the preemption.
- The runner's fixture lock and the firmware flash command take the lease before
  their device and fixture `flock`s, which remain the final exclusion.

A board can be taken out of service: under maintenance it serves only the
owner who took it out; in quarantine it serves nobody. The runner quarantines
a board only when no automatic reset path brings its ROM back to answering,
so no script can make it flashable again; a person resets or power-cycles it
and releases it with `cargo hil devices release MAC --confirm
reset|power-cycle`. A board whose ROM answers the stand's reset is recovered
by reflashing and never quarantined.

The arbiter reports board changes but never restores board state. A board is
identified by the USB serial number of its port, which Espressif USB
Serial/JTAG ports set to the chip's MAC address. The arbiter reads the
stand's boards, their chips, hub ports and UART bridges from the stand file
(`oer-hil-stand-schema`) and never writes it; `Arbiter::at` reads the
`stand.toml` beside its own state.

Checkouts of different versions share the directory. Readers ignore journal
fields they do not know and skip lines they cannot parse; a state with a
newer schema is refused rather than rewritten.

## State

The directory is `~/.cache/open-esp-radio/arbiter`, or `OER_HIL_ARBITER_DIR`.
Every change happens under `arbiter.lock`:

| File | Content |
| --- | --- |
| `state.json` | Schema 3: queue tickets and holders with their claims, each with PID and kernel start time, every recently active owner's balance and the time they were last advanced. Another schema is refused, never converted: a newer one asks for a newer checkout; for an older one, let its holders and waiters finish and remove the file |
| `history.jsonl` | Completed leases: owner, work, scenarios, duration, the time charged, the owner's balance at release, the grant's reason and `released`, `yielded-to-balance`, `hard-limit`, `preempted-on-request` (with who preempted it and why) or `abandoned` |
| `board.jsonl` | Flashes, startup-artifact uploads and writes, automatic recoveries, quarantine releases and soaks, with owner, checkout and board MAC |
| `owners.json` | Schema 1: checkout directory to registered owner; the innermost registered directory decides |
| `maintenance.json` | Schema 1: boards out of service, each with its owner or quarantine, reason, trigger and evidence path |

Each transaction first removes tickets and holders whose process no longer
exists; the start time prevents a recycled PID from keeping a lease alive. The
JSON-lines files keep their newest 2000 records once they exceed 1 MiB.

## Limitations

The arbiter is local to one host and user account and polls every 500 ms; it is
not a distributed reservation. Duration estimates, used only for expected
starts, count only released leases of the identical command line or of single
scenarios. Claims are cooperative: a
command that claims one board and uses another is not detected. Commands that bypass `cargo hil` and `cargo xtask
build firmware --flash`, such as a manual `espflash`, are not ordered unless run
as `cargo hil lease --board NAME -- COMMAND`.
