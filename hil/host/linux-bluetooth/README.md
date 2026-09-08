# Linux Bluetooth DTM fixture

On Linux, build and install the narrow helper from the repository root:

```console
cargo build -p open-esp-radio-hil-runner --bin open-radio-bluetooth
sudo hil/host/linux-bluetooth/install.sh
cargo hil fixture bluetooth-check --adapter hci0
```

Installation grants the invoking operator passwordless access only to the
helper's finite `check --adapter hciN` operation. It does not grant root access
to the general HIL runner. Reinstall after changing the helper. The adapter
must be dedicated to the test: existing connections and a hardware rfkill
block cause rejection before changing its state.

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
