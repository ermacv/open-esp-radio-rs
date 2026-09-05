# Host process ownership

`oer-process` supplies the process lifecycle shared by repository commands and
the HIL runner. It owns direct commands and their ordinary Unix process-group
descendants. Callers own workload policy, arguments, evidence and resource limits.

Install signal handlers once at the application boundary. SIGINT and SIGTERM
set cancellation; new work is rejected and existing waits terminate their owned
process group. `check_cancelled` and `sleep` integrate non-process loops.

`CommandExt` provides bounded status/output probes and owned background children.
Captured stdout and stderr are drained concurrently. Background captures should
set `Child::with_timeout` from the workload duration; waiting later does not
restart that lifetime. `run` preserves the caller's unlimited runtime, with
bounded shutdown. A nested supervisor can request a longer shutdown grace.

`cleanup` temporarily permits restoration despite cancellation. Its 30-second
budget is shared by nested cleanup scopes and limits subprocesses started there.
Leaving the scope restores cancellation, including during unwinding. Arbitrary
blocking code inside a closure is not forcibly interrupted.

The implementation supports Unix process groups. It is not a sandbox: descendants
that create another group/session and processes on a remote SSH host need their
own lifecycle owner. Integration tests use real processes to exercise signals,
pipe capacity, deadlines and descendant cleanup.

```console
cargo test -p oer-process
```
