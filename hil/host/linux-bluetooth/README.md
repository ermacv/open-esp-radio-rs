# Linux Bluetooth fixture

Use the canonical [Linux fixture software installation](../README.md#linux-fixture-software-installation)
route from the repository root:

```console
cargo hil fixture install --provider linux-bluetooth --dry-run
cargo hil fixture install --provider linux-bluetooth
cargo hil fixture bluetooth-check --adapter hci0
```

Installation builds the helper without privileges, then uses one interactive
sudo handoff for versioned activation. The policy defaults to the dedicated
`hci0`; pass a repeated explicit `--adapter hciN` to admit other locally chosen
controllers. The old wildcard adapter grant is not retained. The helper's
finite parser remains the final boundary for peer addresses, hold duration and
termination mode. The unprivileged `capabilities` operation lets the installer
and `cargo hil doctor` reject a stale helper without opening an adapter.

The adapter must be dedicated to the test. Existing connections and a hardware
rfkill block cause an operational check to reject or restore state; installation
itself does not discover, reset or exercise the adapter and performs no RF test.
The required runtime tools are Linux Bluetooth management/HCI interfaces,
`rfkill`, ordinary interactive `sudo` and `visudo`. The installer grants no
access to Cargo, the general HIL runner, itself or a shell.

The helper locks the adapter, snapshots power and soft rfkill, unblocks it,
powers down the kernel controller and acquires the exclusive HCI user channel.
It reads the controller identity and supported commands, then actually runs
DTM v2 RX and TX on LE 1M, channel 0 (2402 MHz), with 100 ms windows. TX uses
37-byte PRBS9 packets. Both runs require successful `LE Test End` responses.
Cleanup resets the controller, releases the user channel and restores and
verifies power and rfkill. Cancellation and errors run the same cleanup;
cleanup failure makes the check fail. Forced termination or device removal
can prevent restoration and must not be interpreted as a passing check.

Each invocation creates a fresh directory under `target/hil/fixture-checks/`
containing `helper.json`, `helper.stderr` and `result.json`. Missing permissions,
rejected commands, timeouts, invalid responses and incomplete cleanup fail
closed. The command needs no ESP, flashing or Wi-Fi lab configuration.

This establishes DTM command acceptance on the selected adapter. It does not
establish RF delivery: `result.json` always reports `rf_verified: false`. A
zero RX count is valid here because this check has no coordinated transmitter.
No qualification claim follows from this fixture check. Other PHYs, extended
advertising and ISO/Audio are outside this finite check.

## Connection loss fixture

With a public-address LE peripheral already advertising, run:

```console
cargo hil fixture bluetooth-connect-reset --adapter hci0 --peer 30:ED:A0:F3:F6:D1
cargo hil fixture bluetooth-connect-reset --adapter hci0 --peer 30:ED:A0:F3:F6:D1 --hold-ms 1000
```

Rebuild and reinstall the helper after updating its command set. The fixture
owns the adapter's exclusive HCI user channel, uses legacy LE initiation with
a 100-ms connection interval, zero latency and a 2-second supervision timeout,
and waits at most 10 seconds for the exact peer's connection completion. It
then sends an exact 251-byte L2CAP-shaped HCI ACL packet. The no-DLE target
declares one 27-byte Host buffer and enables Controller-to-Host flow control.
It receives the packet as ten Link Layer fragments, holds the first consumed
fragment's sole Host credit for 300 ms, returns each credit explicitly,
reassembles the exact payload and returns it as one Host ACL packet. The helper
requires exactly ten returned HCI fragments plus their exact handle, boundary
and reassembled payload within 5 seconds. After the echo, the central requests
an exact 120-ms connection interval with zero latency and the same 2-second
supervision timeout. It requires the matching successful LE Connection Update
Complete within 5 seconds. The helper next marks data channels 2 through 36
bad, polls LE Read Channel Map until the active connection reports the exact
two-channel map, and sends a distinct second 251-byte payload through the full
ten-fragment echo. In
peer-reset mode it then sends HCI Reset without
issuing HCI Disconnect. In peer-rfkill mode it closes the exclusive HCI
channel, soft-blocks the exact adapter through `/dev/rfkill`, verifies the
blocked state before and after 2500 ms, exceeding the requested
2-second supervision timeout. In target-disconnect mode it requires the exact handle
and reason `0x13` from the target's termination PDU. In target-reset mode it
requires supervision-timeout reason `0x08` after the target resets its
Controller. The two target modes never issue peer-side Disconnect or Reset
before observing that result.
`--hold-ms` is bounded
to 0..5000; the default sends Reset as soon as the echo is received.
Both success and failure restore the adapter's original power and rfkill state.

The runner archives the helper report and errors under
`target/hil/fixture-checks/bluetooth-connect-reset-*`. The report separates
connection completion, both exact ACL send/echo exchanges, Connection Update
completion, Channel Map Update application, Reset completion, local elapsed
times and restoration. The ACL and update
fields prove what the central observed over
the selected link. Target-side evidence correlates that exchange with the
production HCI and radio path and determines whether failed establishment or
established-link supervision was exercised. Only peer-rfkill mode sets
`rf_loss_verified`; HCI Reset remains a logical command. This fixture does not
start, flash or reset the ESP.

## ESP and adapter RF scenario

Add the selected adapter to the private `hil/local.toml` configuration:

```toml
[bluetooth]
adapter = "hci0"
```

With the ESP connected at the configured serial port, run:

```console
cargo hil doctor bluetooth-dtm-bidirectional
cargo hil run bluetooth-dtm-bidirectional
cargo hil doctor bluetooth-peripheral-recovery
cargo hil run bluetooth-peripheral-recovery
cargo hil doctor bluetooth-peripheral-soak
cargo hil run bluetooth-peripheral-soak
cargo hil doctor bluetooth-peripheral-local-disconnect
cargo hil run bluetooth-peripheral-local-disconnect
cargo hil doctor bluetooth-peripheral-local-reset
cargo hil run bluetooth-peripheral-local-reset
cargo hil doctor bluetooth-peripheral-rf-loss
cargo hil run bluetooth-peripheral-rf-loss
```

The runner reserves the board and adapter, builds and audits the separate
`bluetooth-dtm` image, flashes it and drives framed, boot-correlated DTM
commands. This image composes the production Bluetooth controller without
the Wi-Fi runtime. It provides Reset, Receive, Transmit and Test End on LE 1M,
channel 0 with the same 37-byte PRBS9 payload as the helper. Active ESP tests
expire after 30 seconds; expiry and HCI errors attempt Reset, retain a failed
state and require a board reset before another test.
USB responses write and flush within one two-second deadline, including
responses whose encoded length is an exact USB packet multiple.

The peripheral recovery scenario starts the target's public `ADV_IND`, asks
the same finite helper to connect as a central, send the exact ACL packet,
validate its echo and reset its local Controller. It then waits for the target's
2-second supervision timeout. Each cycle requires the external central's
Connection Complete, successful LE Read Remote Features and Read Remote Version
Information Command Status events followed by their correlated completions,
the exact feature mask `18:40:00:00:00:00:00:00`, Core version 5.4, company
value `0xffff` and subversion 1, two distinct exact 251-byte ACL echoes separated
by an exact 120-ms Connection Update and applied two-channel map, and a
restoration report. It also requires exactly twenty target Host ACL fragments,
twenty explicit Host credit returns, one deliberate first-credit hold, two
reassembled echo queues and two Host-to-Controller completion credits, one
target-side peripheral
retirement, and ordered standard LE Connection Complete, Connection Update
Complete and Disconnection Complete events. The
disconnect status must be successful, the sole profile handle must be `0x0001`,
and the reason must be `0x08`. The catalog runs two cycles per boot and three
fresh repetitions, proving that advertising can restart after the first idle
restoration. A passing physical run establishes the tested bounded RX
backpressure and bidirectional legacy fragmentation path. Concurrent logical
ACL packets, sustained throughput, DLE and GATT remain outside this scenario.
The separate `bluetooth-peripheral-soak` scenario applies the same exact gates
to 100 sequential connections in one boot and one catalog repetition. It is a
bounded recurrence test, not a throughput or duration qualification.
The local-disconnect scenario requires a successful target Disconnect Command
Status, local Disconnection Complete reason `0x16`, peer reason `0x13` and a
second advertising/connection cycle. The local-reset scenario requires target
Reset Command Complete, peer supervision timeout and complete Host bootstrap
reconfiguration before the second advertising cycle. These are logical HCI
lifecycles; neither claims powered RF teardown or cold reconstruction.
The RF-loss scenario applies the same ACL/update gates, then verifies that the
Linux peer remains rfkill-blocked for at least 2500 ms. The target must report supervision
timeout reason `0x08`, restore advertising and complete a second connection.

Test End and logical Reset share bounded production scheduler stop and exact
descriptor retirement. A deadline or ownership fault retains the graph and
fails the command; it never reports successful cancellation or reclaims
hardware-owned memory. The quiet controls exercise this path without packets.

The scenario first holds ESP TX while the helper counts packets on the PC.
With the PC adapter restored and inactive, each quiet cycle checks RX/Test End
and RX/Reset/RX/Test End, using 300-ms receive windows without a board reset.
The workload's `quiet_cycles` accepts 1 through 1000; the catalog requests 100
per boot. Omitting it preserves three End controls and one Reset restart.
Every Test End count must be zero and every
Reset restart must finish. Partial counts are retained on failure.
Finally ESP remains in RX throughout another helper invocation: the helper's
RX window measures PC silence, and its TX window supplies packets to ESP.
Thus each receiver has both a positive-delivery requirement and a zero-count
control with the other transmitter inactive. This is not an idle-state RF
emissions test or a packet-error-rate measurement.

Both positive counts must reach the scenario's `minimum_packets`, and both
silence counts must equal zero.
DTM events retain cumulative production RX and sequence-deadline diagnostics
in `protocol.jsonl`. These observations do not acknowledge hardware or change
packet accounting. Command acceptance alone cannot pass. Every
completed boot writes `bluetooth-dtm.json`, peer helper reports and the framed
UART transcript into the ordinary immutable HIL bundle, including on failure.
ESP Reset and adapter restoration are checked independently of RF acceptance;
the peer identity must match across directions. The catalog scenario requests
two boots and stops at the first failure. It establishes
only the tested LE 1M DTM link; general BLE readiness remains with qualification.
