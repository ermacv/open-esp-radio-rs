# Host process ownership

`oer-process` supplies the process lifecycle shared by repository commands and
the HIL runner. It owns direct commands and their ordinary Unix process-group
descendants. Callers own workload policy, arguments, evidence and resource limits.

Install signal handlers once at the application boundary. SIGINT, SIGTERM and
SIGHUP set cancellation; new work is rejected and existing waits terminate their
owned process group. `check_cancelled` and `sleep` integrate non-process loops.

An owner that dies without that cleanup (SIGKILL, an abort) does not leave its
children running: the first owned spawn forks a small guardian process that
tracks every owned process group through a close-on-exec pipe, and when the
pipe closes with the owner it terminates the groups still registered (SIGTERM,
then SIGKILL after a second). `run_with_timeout` bounds a command's whole
lifetime, as the push gate does for test runs.

`CommandExt` provides bounded status/output probes and owned background children.
Captured stdout and stderr are drained concurrently. Background captures should
set `Child::with_timeout` from the workload duration; waiting later does not
restart that lifetime. `run` preserves the caller's unlimited runtime, with
bounded shutdown. A nested supervisor can request a longer shutdown grace.
`exit_code` maps a child's status to the code a wrapper exits with: the
child's own code, or `128 + signal` for one a signal ended.

`cleanup` temporarily permits restoration despite cancellation. Its 30-second
budget is shared by nested cleanup scopes and limits subprocesses started there.
Leaving the scope restores cancellation, including during unwinding. Arbitrary
blocking code inside a closure is not forcibly interrupted.

Three more jobs have their one owner here:

- `git`: every repository tool runs Git as `git -C <directory> …` through
  `git::output`, `git::text`, `git::lines`, `git::run` or, for extra
  arguments or environment, `git::command`; a failure carries Git's error
  output.
- `Checkout` and `built_root`: the checkout a tool acts on. A tool is built
  from a checkout and this package is its path dependency, so
  `built_root()` is the tree the binary came from, and
  `Checkout::discover(tool)` refuses to run inside another checkout
  (`--root` selects one explicitly). Cargo is started through
  `oer-toolchain`.
- `proc`: a live process's start time (`start_ticks`, `started_unix_millis`)
  from Linux `/proc`, which tells a recycled PID from the process that held
  it.

The implementation supports Unix process groups. It is not a sandbox: descendants
that create another group/session and processes on a remote SSH host need their
own lifecycle owner. Integration tests use real processes to exercise signals,
pipe capacity, deadlines and descendant cleanup.

```console
cargo test -p oer-process
```
