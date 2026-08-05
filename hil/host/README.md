# HIL host fixtures

`runner/` owns unprivileged scenario orchestration and typed UART capture.
`linux-net/` owns the minimal privileged host networking operations required
by those scenarios. Neither directory contains reusable driver behaviour.

Host fixture defaults are qualification policy. Board credentials, network
interface selection and local device paths must not leak into production
crates.

The installer derives the repository root from its own location and installs
the already-built HIL runner with only `cap_net_raw`. No checkout path is
embedded in the privileged helper.
