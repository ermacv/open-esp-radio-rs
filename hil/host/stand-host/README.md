# HIL stand host

`oer-hil-stand-host` covers the host around the stand file:

| Module | Owns |
| --- | --- |
| `discover` | `cargo hil stand discover`: every Espressif board `uhubctl` reports against the stand file (in place, moved, missing with why, new with a `[[board]]` fragment), `--blink` and `--verify-power` under a lease |
| `doctor` | `cargo hil stand doctor`: the stand file and its chip profiles, the udev rule, `uhubctl` without sudo, NetworkManager leaving `wlan0` alone |
| `fixtures` | `cargo hil fixtures` and the dashboard: the host's radios and Bluetooth adapter and the OpenWrt hosts, each with its lease key, whether it answers, its interfaces, channels and CCA busy share |
| `ssh` | The one way the host reaches the stand's OpenWrt hosts (non-interactive, five-second connect timeout) and the lease key of an OpenWrt host's boot |

The runner's Wi-Fi fixtures, the run's fixture lock and the lab provenance
reach OpenWrt through `ssh`. Fixture software installation, which runs as
root, stays in [`oer-hil-fixture-install`](../fixture-install), whose whole
dependency graph is the root-executed surface.

```console
cargo test -p oer-hil-stand-host
```
