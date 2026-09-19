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

### Explicit DTM v1 diagnostic

For a controlled comparison of command versions on the same adapter, select
the v1 profile explicitly:

```console
cargo hil fixture bluetooth-check --adapter hci0 --dtm-version v1
```

The default remains v2. Both profiles use LE 1M, channel 0, 100 ms RX/TX
windows, 37-byte PRBS9 TX packets and the same two-second command deadline.
The selected profile must advertise its RX, TX and Test End commands. A missing
command, rejection or timeout fails the check; the helper never retries or
switches versions. Reset, exclusive adapter ownership and verified restoration
are unchanged. Reinstall with `cargo hil fixture install --provider linux-bluetooth`
after updating its interface; installation adds only an exact v1 check grant for each admitted
adapter, not arbitrary command or timeout access.

Check report schema 2 records `dtm_version` and both advertised command profiles.
The runner requires the reported version to match the requested version.
DTM RF and watchdog scenarios continue to require v2; a passing diagnostic v1
check is not substituted for their peer evidence. The helper uses a local typed
TX v1 command correction because the pinned `bt-hci 0.10.1` assigns that command
the Read Supported States opcode; its parameters and completion handling still
use `bt-hci`. Socket tests exercise the corrected TX command bytes.

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

`rf_loss_verified` proves the helper's observed adapter block and hold, not
an abrupt over-the-air outage. Closing the exclusive user channel first lets
Linux close the controller, which may terminate the link before rfkill.
The target can therefore receive remote termination `0x13` instead of
supervision timeout `0x08`. The RF-loss scenario still requires `0x08` and
fails in that case. This software-only stimulus is not sufficient to qualify
abrupt RF loss on an adapter that terminates gracefully during close.

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
connection retirement and advertising recovery. Each cycle requires the external central's
Connection Complete, successful LE Read Remote Features and Read Remote Version
Information Command Status events followed by their correlated completions,
the exact negotiated feature mask `19:40:00:00:00:00:00:00` (Encryption,
Peripheral Feature Exchange, LE Ping and CSA #2), Core version 5.4, company
value `0xffff` and subversion 1, two distinct exact 251-byte ACL echoes separated
by an exact 120-ms Connection Update and applied two-channel map, and a
restoration report. It also requires exactly twenty target Host ACL fragments,
twenty explicit Host credit returns, one deliberate first-credit hold, two
reassembled echo queues and two Host-to-Controller completion credits, one
target-side peripheral
retirement, and ordered standard LE Connection Complete, Connection Update
Complete and Disconnection Complete events. The
disconnect status must be successful, the sole profile handle must be `0x0001`,
and this fixture profile accepts reason `0x13` (remote-user termination) or
`0x08` (supervision timeout), retaining the actual reason in the target evidence.
HCI Reset can optionally transmit `LL_TERMINATE_IND`; see Bluetooth SIG
[HCI Test Suite HCI/DSU/BV-06-C, Figure 4.9](https://files.bluetooth.com/wp-content/uploads/dlm_uploads/2025/05/HCI.TS_.p37.pdf#page=36).
Timeout itself is qualified only by the separate RF-loss scenario.
The catalog runs two cycles per boot and three
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

The connection helper report uses schema 14. The installer and runner require
the same advertised helper contract; reinstall the Linux Bluetooth fixture when
that contract changes. Advertising Encryption in this exchange does not qualify
encrypted traffic: key exchange, MIC/counter handling and encrypted interoperability
require their own evidence.

## Kernel ATT calibration fixture

The `bluetooth-peripheral-acl-calibration` workload uses a fixed ATT socket
through the Linux kernel and BlueZ, implemented in
[`fixture/bluetooth/att.rs`](../runner/src/fixture/bluetooth/att.rs). It uses the
same runner adapter lease but does not enter the helper's exclusive HCI channel.
The configured adapter must initially be powered off. BlueZ, `busctl` and user
access to `/dev/rfkill` are required; the socket binds the selected adapter's
public address. Missing access fails setup rather than changing permissions.

The owner snapshots adapter identity and soft rfkill, powers it for the test,
and verifies power/rfkill restoration on completion or error. The brief BlueZ
re-registration interval after clearing rfkill has a bounded setup retry. Peer
payloads, HCI credit snapshots, calibration counts and cleanup results are saved
with the ordinary sealed HIL run. No helper reinstall is required for this path.


## Encrypted peripheral ACL fixture

`cargo hil run bluetooth-peripheral-encrypted-acl` uses the same exclusive
Linux HCI fixture and adapter lease as peripheral recovery. Reinstall the helper
with `cargo hil fixture install --provider linux-bluetooth --adapter hci0` after
updating its schema. Its finite `connect-reset --encrypted` mode starts AES-CCM
with the public test LTK/Rand/EDIV from `open-esp-radio-hil-protocol`, requires
successful Command Status followed by Encryption Change for the exact handle,
and only then sends application data. No arbitrary keys or HCI commands are
accepted by the launcher interface.

Each connection checks both exact 251-byte echoes, with the second following
Connection Update and Channel Map Update. The current target fragments its
outbound encrypted payload into eleven LL fragments; the central's existing
27-byte payload profile produces ten target Host fragments per exchange.
Target evidence requires one matching LTK request/reply and Encryption Change,
no plaintext application data, complete credit returns and link retirement.
Two connections share one target boot and HCI epoch; three repetitions each end
in cold retirement. Peer Reset is still the recovery stimulus, not abrupt RF
loss. Adapter state is restored on success or failure.

`cargo hil run bluetooth-peripheral-key-refresh` selects the same workload with
`key_refresh = true`. The finite helper mode `connect-reset --encrypted
--key-refresh` issues LE Enable Encryption again after both updates, with the
distinct public `BLUETOOTH_REFRESH_*` identity. Before sending the second echo it
requires a new successful Command Status and Encryption Key Refresh Complete on
the original handle. The target requires two matching key requests/replies, one
initial Encryption Change and one Key Refresh Complete per connection; missing
or duplicate transitions fail the scenario. Both connection cycles use the same
Controller epoch. The helper accepts no arbitrary key material.

These scenarios cover fixed-key Controller interoperability. Pairing, bonding
and intentionally corrupted MIC tests remain separate requirements;
a passed plaintext scenario or an advertised feature bit cannot substitute for
this evidence.

## Key failure fixtures

`cargo hil run bluetooth-peripheral-missing-key` and
`cargo hil run bluetooth-peripheral-wrong-key` use the same leased helper.
Its finite `security-failure --adapter hci0 --peer <address> --failure
missing-key|wrong-key|missing-refresh-key|active-data-mic` command accepts no custom key or opcode. Install it through
`cargo hil fixture install --provider linux-bluetooth --adapter hci0`.

The target diagnostic Host injects exactly one negative LTK reply or one reply
with a mismatched public key. The peer sends no application data on this failed
connection. Missing key requires a successful encryption-command admission
followed by Encryption Change with PIN or Key Missing (`0x06`, encryption off).
The helper then explicitly disconnects this still-live plaintext ACL, requiring
local reason `0x16` and target remote-user reason `0x13`. It does not misclassify
initial key rejection as an automatic disconnect.

`cargo hil run bluetooth-peripheral-missing-key-plaintext` is a separate
diagnostic. Its fixed `--read-version-before-disconnect` helper flag requests
Read Remote Version Information after the `0x06` rejection and requires a
successful completion for the same handle and the target's exact development
identity before sending Disconnect. The original missing-key scenario still
disconnects immediately. Both retain target reason `0x13` and the same recovery
requirements; the diagnostic is not a substitute in qualification. A version
completion may come from the central's cache and alone does not prove a fresh
over-the-air exchange or application-data progress.

`cargo hil run bluetooth-peripheral-missing-refresh-key` first requires a
successful initial Encryption Change on the same handle, then requests the
distinct replacement LTK. The target Host rejects that second key request.
Both endpoints must observe termination with PIN or Key Missing (`0x06`);
the helper never sends Disconnect to manufacture that result. A failed Key
Refresh Complete may precede disconnect and must also carry `0x06`; successful
refresh, supervision timeout and application delivery fail the probe. The
initial and refresh command admissions are recorded separately. This finite
mode requires the current helper capability contract and matching installer policy.

`cargo hil run bluetooth-peripheral-active-data-mic` requires successful initial
encryption, then sends the fixed ACL payload. The target's explicit diagnostic
feature corrupts one received data MIC before production authentication. It
must report exactly one injection, no Host ACL delivery and reason `0x3d`.
The peer must observe timeout `0x08` after sending data, without a local
Disconnect or any received application data. The scenario then requires a fresh
encrypted connection with exact echoes and cold retirement. This tests the
target's response to corrupted RX input, not transmission of a bad MIC over RF.
The finite helper mode requires schema 16 and matching installation rules.

Security-failure reports use schema 4 and retain at most 64 incoming HCI
packets, each bounded to 258 bytes, with monotonic times relative to submission
of LE Enable Encryption. This chronology is not an RF capture. The complete
probe retains its eight-second deadline, including the optional version read;
missing or failed completion does not fall back to immediate Disconnect.

Wrong key requires target MIC failure `0x3d`, with no successful encryption or
Host ACL delivery. The central observes supervision timeout `0x08` after the
target stops the failed link; any Encryption Change received before disconnect
must also report failure `0x08` with encryption off. That central timeout alone
cannot satisfy the scenario. A spontaneous disconnect, rejected HCI command,
successful encryption, unrelated handle, data packet or incomplete restoration
fails the probe.

Each scenario then reconnects in the same target boot/HCI epoch, starts normal
encryption and checks both exact fragmented echoes, connection/channel-map
updates, peer Reset recovery and cold retirement. There are three repetitions.
Preflight checks password-free admission of all three failure modes and the separate
version diagnostic using an invalid peer
address, rejected by the CLI parser before any adapter acquisition. A successful
`sudo -l` listing alone is not proof of password-free execution.
Reports retain both peer evidence and target snapshots, including partial
failures. A failed peer command triggers a bounded target snapshot before the
capture closes; a failed snapshot is reported separately and never replaces the
original error. Refresh completion errors retain the exact peer HCI status and
actual/expected handles. These probes cover initial-key rejection, missing refresh keys and handshake
MIC failure; they do not establish corrupted data MIC handling in an already
encrypted session, SMP, pairing or secure GATT.

## Secure Trouble GATT fixture

Run `cargo hil run bluetooth-trouble-secure-gatt` from the repository root;
no interactive terminal or stdin response is required, including over SSH.
It uses the dedicated, initially powered-off adapter and ordinary kernel/BlueZ
path, not the helper's exclusive HCI channel. No helper reinstall is needed for
this path. BlueZ D-Bus access, `busctl`, `/dev/rfkill` access and a DUT without an
existing BlueZ device record are required. A preexisting record is never deleted
or adopted, even if it is unpaired or left after an interrupted earlier run.

The fixture registers its own `DisplayYesNo` agent on the same bus connection
that issues `Pair`; it does not replace the default desktop agent. It accepts
callbacks only from that BlueZ owner for the selected DUT and rejects other
pairing methods. BlueZ restart invalidates ownership. Read/write/notification
verification uses BlueZ GATT; notifications are received through `AcquireNotify`,
not inferred from a cached characteristic value.

The scenario first verifies that plaintext value reads/writes and CCCD writes
receive security errors. It then explicitly declines one Numeric Comparison
attempt. The positive attempt compares the independent numbers received from
the DUT's framed USB console and the selected BlueZ agent callback. Only a match
produces explicit positive replies to both peers; the DUT reply is bound to the
exact boot and pending request. Missing, invalid, stale or different challenges
cannot be accepted. This is an automated HIL test operator, not proof of human
presence or a production auto-pairing policy. The standalone application's
manual console remains unchanged. The agent callback has a 25-second limit and
D-Bus calls a 35-second limit.

After authenticated enrollment it checks value reads/writes and independently
received notifications, disconnects, and repeats using the retained bond without
a new comparison. It then requests full physical Controller cold shutdown and
restart while retaining the application's RAM store. The SoC boot must remain
unchanged, the old HCI handle must be closed, and a fresh Host must resume
authenticated encryption without pairing. The characteristic resets to zero;
the bond does not. The DUT has one RAM-only bond slot. BlueZ may temporarily
persist the peer bond on Linux; cleanup disconnects, removes only the record
whose absence was checked before this experiment, verifies removal, unregisters
the agent, and restores power/rfkill. Any cleanup error prevents PASS. Forced
process death can leave a record: the next run refuses it rather than silently
deleting unknown state. Review that exact record before manually removing it.

`trouble-secure-gatt.json` retains value-only snapshots, independent peer
observations, comparison identities, the automated confirmation mode and cleanup
results. No LTK/IRK is exported.
The source scenario is not hardware qualification. It does not prove power-loss
bond persistence, storage-fault recovery, RF quality or terminal-fault/cancellation
disposition during retirement.
