# HIL stand arbiter

`oer-hil-arbiter` orders access to the HIL stand between every checkout of one
host user. The [host guide](../README.md#share-the-stand) describes the
commands; this crate owns the queue, leases, claims, budgets and the board
journal.

## Ownership

- A request claims resources, each exclusively or shared: `board:<MAC>`,
  fixture keys, `air`, or `stand`, which conflicts with everything and is the
  claim of a request that names nothing. Holders whose claims do not conflict
  hold leases at once.
- `Arbiter::acquire` enqueues a ticket and waits until no holder and no
  earlier waiting ticket conflicts with it. A short ticket (budget at most two
  minutes) may pass earlier conflicting tickets, but never two grants in a row.
  A process that already holds a lease, or carries its token in
  `OER_HIL_LEASE`, joins it without waiting when the lease covers its claims;
  a token of an ended lease or a claim outside the lease is an error.
- `Grant` releases the lease on drop and records it in the history with its
  outcome. `supervise_self` makes the holder its own watchdog: within its
  budget, divisible work sees `yield_requested` when a waiting ticket with a
  budget of at most five minutes (`BRIEF_BUDGET`) conflicts with it; past its budget,
  while a waiting ticket conflicts with it, divisible work sees
  `yield_requested` and yields at its next boundary; indivisible work receives
  `SIGTERM`. At twice the budget it receives `SIGTERM` regardless. `cargo hil
  lease` supervises its command group the same way.
- The runner's fixture lock and the firmware flash command take the lease before
  their device and fixture `flock`s, which remain the final exclusion.

The arbiter reports board changes but never restores board state. A board is
identified by the USB serial number of its port, which Espressif USB
Serial/JTAG ports set to the chip's MAC address. The runner registers the
board it flashes as `esp32s31` when its chip is unknown; other chips are registered by `cargo hil devices set` or a
recorded flash with `--chip`.

Checkouts of different versions share the directory. Readers ignore journal
fields they do not know and skip lines they cannot parse; a state or registry
with a newer schema is refused rather than rewritten.

## State

The directory is `~/.cache/open-esp-radio/arbiter`, or `OER_HIL_ARBITER_DIR`.
Every change happens under `arbiter.lock`:

| File | Content |
| --- | --- |
| `state.json` | Schema 2: queue tickets and holders with their claims, each with PID and kernel start time. Schema 1 (one whole-stand holder) is migrated once its holder and waiting processes have ended; newer requests wait until then |
| `history.jsonl` | Completed leases: owner, work, scenarios, duration, budget and `released`, `yielded`, `preempted`, `budget-exceeded` or `abandoned` |
| `board.jsonl` | Flashes and startup-artifact uploads and writes, with owner, checkout and board MAC |
| `devices.json` | Schema 1: board MAC to chip and name; boards have no fixed role |

Each transaction first removes tickets and holders whose process no longer
exists; the start time prevents a recycled PID from keeping a lease alive. The
JSON-lines files keep their newest 2000 records once they exceed 1 MiB.

## Limitations

The arbiter is local to one host and user account and polls every 500 ms; it is
not a distributed reservation. Budget estimates count only released leases of
the identical command line or of single scenarios. Claims are cooperative: a
command that claims one board and uses another is not detected. Commands that bypass `cargo hil` and `cargo xtask
build firmware --flash`, such as a manual `espflash`, are not ordered unless run
as `cargo hil lease --board NAME -- COMMAND`.
