# Linux network fixture

Use the canonical [Linux fixture software installation](../README.md#linux-fixture-software-installation)
route from the repository root:

```console
cargo hil fixture install --provider linux-net --dry-run
cargo hil fixture install --provider linux-net
```

`cargo hil fixture install-host` is retained only as an alias for the second
command. Do not run `sudo install.sh`; the tracked script delegates to the same
Cargo workflow only when invoked as the unprivileged operator.

Preparation requires Linux, Cargo, a C compiler, make, pkg-config, libnl3 and
OpenSSL development files, curl, tar and patch. It builds the pinned
[hostapd input](hostapd/README.md), the finite probe helper and the versioned
network helper without privileges. Interactive sudo is used only for the final
root-owned staging, policy validation and activation.

The provider is deliberately restricted to `wlan0`. Installing it does not
discover or assign adapters, change network state, start hostapd, reset rfkill,
open serial/SSH, flash a DUT or transmit RF. Those effects belong to explicit
fixture checks and HIL runs described in the [host operations guide](../README.md).
