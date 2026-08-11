# HIL host fixtures

`runner/` owns unprivileged scenario orchestration and typed UART capture.
`linux-net/` owns the minimal privileged host networking operations required
by those scenarios. Neither directory contains reusable driver behaviour.

`../local.toml` owns all machine-local values. The runner learns the target
address from typed `NetworkReady` evidence and the reverse-flow host address
from the kernel route to that target. RX qualification captures the same UDP
session at the OpenWrt DSA ingress and Wi-Fi egress and rejects SSH loss or
capture drops. DSA ingress is diagnostic because switch offload can bypass the
host packet socket; Wi-Fi egress is the exact AP admission edge.

The installer derives the repository root from its own location and installs
the already-built HIL runner with only `cap_net_raw`. No checkout path is
embedded in the privileged helper.
