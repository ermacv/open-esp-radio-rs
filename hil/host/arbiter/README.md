# HIL stand arbiter

`oer-hil-arbiter` orders access to the single physical HIL stand between every
checkout of one host user. The [host guide](../README.md#share-the-stand)
describes the commands; this crate owns the queue, leases, budgets and the
board journal.

## Ownership

- `Arbiter::acquire` enqueues a ticket and waits until it is granted. A process
  that already holds the lease, or carries its token in `OER_HIL_LEASE`, joins
  it without waiting; a token of an ended lease is an error.
- Grants follow the queue order. A short ticket (budget at most two minutes)
  may be granted ahead of the head, but never twice in a row.
- `Grant` releases the lease on drop and records it in the history.
  `terminate_self_on_overrun` makes the holding process its own watchdog;
  `cargo hil lease` supervises its command group instead.
- The runner's fixture lock and the firmware flash command take the lease before
  their device and fixture `flock`s, which remain the final exclusion.

The arbiter reports board changes but never restores board state.

## State

The directory is `~/.cache/open-esp-radio/arbiter`, or `OER_HIL_ARBITER_DIR`.
Every change happens under `arbiter.lock`:

| File | Content |
| --- | --- |
| `state.json` | Schema 1: queue tickets and the holder, each with PID and kernel start time |
| `history.jsonl` | Completed leases: owner, work, duration, budget, `released`, `budget-exceeded` or `abandoned` |
| `board.jsonl` | Flashes and startup-artifact uploads and writes, with owner and checkout |

Each transaction first removes tickets and holders whose process no longer
exists; the start time prevents a recycled PID from keeping a lease alive. The
JSON-lines files keep their newest 2000 records once they exceed 1 MiB.

## Limitations

The arbiter is local to one host and user account and polls every 500 ms; it is
not a distributed reservation. Budget estimates count only completed leases of
the identical command line. Commands that bypass `cargo hil` and `cargo xtask
build firmware --flash`, such as a manual `espflash`, are not ordered unless run
as `cargo hil lease -- COMMAND`.
