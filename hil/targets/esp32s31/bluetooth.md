# Bluetooth HIL target

## Trouble applications

`bluetooth-gatt` is a separate plaintext Trouble Host image. It compiles the
actual [standalone application](../../../examples/esp32s31-bluetooth-controller/src/gatt.rs),
not the diagnostic ATT responder. The `bluetooth-trouble-gatt` scenario uses
the Linux fixed-ATT fixture to discover service/characteristic handles, read,
write and reconnect three times on the same Host/Controller epoch. USB exposes
only application observations, boot identity and transport/stack diagnostics;
it cannot submit HCI commands. The image omits DTM and RX fault-injection
features. Pairing, security policy, bond persistence and coordinated cold
shutdown are outside this plaintext baseline. Runs require the usual installed
Bluetooth fixture and an initially powered-off selected Linux adapter.

`bluetooth-secure-gatt` instead compiles the standalone `security::gatt::run`
application with a caller-owned one-slot RAM bond store. Its framed console
accepts observations, explicit Numeric Comparison decisions and an epoch-bound
Controller restart request, a one-shot Reset-reader gate and a one-shot failure
of the next bond-store load; it cannot
drive HCI, reply to ATT or install keys. Plaintext and secure applications share
board/Controller construction but have separate application and evidence modules.
For restart, `security::epoch::run` drops the application, drains any outstanding
bootstrap Reset with the existing Host runner, then drops that runner, consumes
the Host and waits for a separate HCI Reset completion with an independent reader.
The hardware runner then returns idle; checked timer/HCI/IRQ/output/platform
retirement and physical PHY close precede cold restart. Fresh Host resources are
reborrowed while the application retains its RAM bond and comparison sequence.
An application or Host failure reports `application_stopped`. If the final Reset
succeeds, checked cold release closes the physical Controller and retains the
cold owners and original failure without restart. An unacknowledged bootstrap
Reset, failed drain or failed final Reset instead retains the physical runner;
it does not authorize release, restart or silent re-enrollment. The inherited
hardware fault policy is unchanged. The store-fault adapter belongs only to the
HIL application composition; the standalone RAM store and radio are unchanged.
The HIL-only HCI wrapper delegates every operation to the original facade. When
armed while disconnected, it suspends the sole shutdown reader before response
consumption, leaving the real command future and hardware runner alive. The
reached checkpoint must retain the old epoch and physical owners. Explicit
release wakes the same reader and permits normal retirement only after the real
Reset response is consumed. This tests pending-Reset retention, not a silicon
stall or RF cessation. Transport-error retention is a separate scenario.

Run `cargo hil run bluetooth-trouble-secure-gatt` from the repository root.
The Linux peer exercises denied plaintext ATT, an explicit negative pairing,
then automatically compares the independent DUT/BlueZ Numeric Comparison numbers
and sends exact boot/request-bound decisions to both peers. No terminal input is
needed. This test operator does not prove human presence and does not change the
standalone application's explicit user-consent policy.
Successful pairing is followed by read/write/notify and encrypted
reconnect without another confirmation, followed by full Controller cold
shutdown/restart and another encrypted reconnect. Before that cold restart,
the scenario holds the shutdown reader for a one-second observation interval;
it requires no cold release, epoch change, shutdown success or SoC reset, and
rejects a duplicate restart. The host then releases the reader and requires unchanged
SoC boot identity, a new application epoch, physical cold release and rejection
of both commands and reads on the old HCI handle. The fresh GATT value starts at
zero; the bond remains in application RAM. Finally, while connected, the test
arms a single backend read failure and disconnects the peer. The unchanged GATT
application reloads its store and fails; the host requires that specific cause,
a successful final Reset, checked physical close, rejection of restart and an
unchanged boot/epoch throughout a one-second observation window. That window is
not a production timeout or RF-stop bound. The Linux bond is temporary and
removed at cleanup; SoC reset or power loss clears the DUT bond. See the
[secure fixture contract](../../host/linux-bluetooth/README.md#secure-trouble-gatt-fixture).
The scenario's existence and a successful build do not qualify RF behavior.

`cargo hil run bluetooth-trouble-secure-gatt-hci-read-failure` runs the same
pairing, protected traffic and RAM-bonded reconnect checks, including normal cold
restart. It then disconnects the peer, holds a new shutdown Reset reader and
injects a distinct read error at that reached checkpoint. The real Host epoch
must preserve its requested-stop cause and return `Retain`; the host requires
`InjectedReceiveFailure`, unchanged epoch/cold-release count and boot, and rejection
of another restart throughout a one-second observation interval. The last
`old_hci_closed` value still describes the preceding successful cold release,
not closure of the failed epoch. No read response is discarded, and the physical
runner and owners remain retained. Fixture/DUT cleanup is outside the production
shutdown proof. This scenario does not establish RF-off or test a lost silicon
interrupt. The two scenarios select distinct workload kinds; the existing
secure-GATT workload shape remains readable in sealed runs. One successful terminal result cannot stand
in for the other.

## Controller diagnostics

The separate `bluetooth-dtm` image selects `bluetooth-hil`, the production
Bluetooth composition and the framed HIL control protocol without a Wi-Fi
network feature. Its USB owner translates bounded DTM requests into typed HCI
commands; radio execution remains in production crates. The same image exposes
the bounded [peripheral diagnostic commands](../../protocol/README.md) for
starting a single-channel connectable advertisement and observing execution.
They require a fresh boot. After an observed disconnect or failed establishment,
advertising may restart without a board or HCI Reset; the host resets the board
when the probe ends. The commands alone do not qualify a connection or ACL
traffic. The `bluetooth-peripheral-recovery` scenario coordinates a real Linux
central with two connection, ACL echo, Connection Update, Channel Map Update
and peer Reset recovery cycles. The target Host
declares one 27-byte Controller-to-Host ACL credit, holds the first consumed
fragment's credit for 300 ms, returns every credit explicitly, requires all ten
fragments of an exact 251-byte ACL packet, reassembles and echoes it through the
target Host facade, requires the central to complete LE Read Remote Features
with the [shared fixture feature contract](../../host/fixture-install/src/bluetooth_contract.rs)
and Read Remote Version Information with Core
5.4, company value `0xffff` and subversion 1 after their successful Command
Status events, requests an exact 120-ms interval and a two-channel map from the
central,
then sends a distinct second 251-byte exchange after the map applies. Each target
echo must also produce Number Of Completed Packets after Link Layer acknowledgement. It
checks the matching Host-visible Connection Update Complete between connection
and disconnection before each advertising restart. The source-backed scenario
has no recorded hardware evidence by itself. See the
[Bluetooth fixture and RF scenario](../../host/linux-bluetooth/README.md).
The separate `bluetooth-peripheral-soak` catalog entry repeats the same exact
cycle 100 times in one boot; it has the same evidence limitation until run on
hardware.
The `bluetooth-peripheral-local-disconnect` and
`bluetooth-peripheral-local-reset` entries apply the same ACL/update/map/second-ACL gates, then
trigger the selected command from the target Host. Disconnect requires local
reason `0x16` and peer reason `0x13`; Reset requires peer supervision timeout
and reruns the bounded Host bootstrap before advertising is restarted. These
scenarios do not exercise powered teardown.
The target Host continues pumping HCI events while awaiting its own Disconnect
or Reset command response, so command completion and unsolicited events cannot
block each other.
The `bluetooth-peripheral-rf-loss` entry instead closes the Linux central's HCI
channel and keeps its radio rfkill-blocked for at least 2500 ms after the second ACL exchange. It requires target supervision
timeout reason `0x08` and advertising recovery before the next connection.

The `bluetooth-peripheral-retirement` scenario ends real connection cycles in
physical cold ownership and probes all old HCI authorities for closure.
`bluetooth-peripheral-powered-restart` makes three connections per boot, with
full cold release and restart on the original allocations between connections.
`bluetooth-peripheral-phy-maintenance` uses the same three-connection profile
but services due PHY tracking while preserving the powered epoch and HCI.
Both check a working HCI Reset after each transition and finish with physical
cold retirement. Run with `cargo hil run <scenario>` from the repository root,
using the installed Linux Bluetooth fixture and local lab configuration.
Maintenance admission is idle-only; these scenarios do not establish automatic
tracking during a continuously active connection or DTM session.

`bluetooth-peripheral-phy-calibration` uses the same idle handoff with
`calibration_threshold = 0`. It requires actual common RX and Bluetooth TX
calibration completions between connections, same-HCI Reset and final cold
release. The temporary threshold never substitutes sensor readings and is
restored before the next connection. The `bluetooth-dtm` image selects the
`phy-rx-hot-sram` placement experiment for direct RX-gain without enabling
Wi-Fi; sealed older images retain their original feature report and firmware
provenance.

`bluetooth-peripheral-active-phy-maintenance` uses automatic production ACL
handoff. Each connection must contain new physical maintenance and matching
guarded RUN completions, followed by ACL/update/map progress and peer Reset
recovery. Its explicit execution/restoration/deferral values remain engineering
budgets. It does not force every expensive calibration branch or prove the
shortest connection interval. Run either scenario with `cargo hil run <scenario>`.

`bluetooth-peripheral-encrypted-phy-maintenance` uses that automatic image
with the fixed-key Host and two connections per boot. Live snapshots must show
bidirectional encrypted ACL progress, a subsequent guarded PHY restoration
without changing the connection or encryption session, then new received data
and acknowledged Host transmission. The independent central also requires both
exact echoes. Peer Reset recovery and final cold retirement remain mandatory.
This scenario uses engineering budgets and does not force every calibration
branch or establish thermal/RF-quality bounds.

`bluetooth-peripheral-maintenance-backpressure` requires new guarded PHY
restoration on a live plaintext ACL connection before the Host withholds an
RX credit. HCI event reads continue through supervision expiry; the old credit
must drain before same-HCI reconnect and final physical cold retirement.
It combines active maintenance with the existing backpressure lifecycle using
the automatic maintenance image. It does not inject lost IRQs or hardware faults.

`bluetooth-peripheral-active-data-mic` uses the plain Bluetooth image and its
explicit `rx-fault-injection` feature. The diagnostic corrupts one received data
MIC after encryption starts. Production authentication must reject all Host
delivery and retire the connection with `0x3d`; subsequent encrypted recovery
must use fresh session state and leave the one-shot injection disarmed. The
peer sends valid encrypted data: this is RX-input fault injection, not proof
of an invalid MIC on the air. Ordinary production builds omit this feature.

`phy_maintenance` evidence contains boot totals and child timing maxima plus
the latest transaction's timestamps. An idle transaction has no restoration
deadline or RUN; it preserves the totals of earlier guarded ACL restorations.
`latest_rx_quality` identifies the last completed RX DC product and remains
available across subsequent light tracking. Wi-Fi's `rx_gain.execution.quality`
identifies the DC product of that observed transaction. Per-gain false values
mean iteration-limit completion: baseband searches retain their initial pair,
while radio/fine searches retain their last correction. Missing quality differs
from a completed product with limited searches. Lifecycle scenarios require the
observation when forcing calibration, but do not require every search to
converge or claim RF quality. See the
[PHY result contract](../../../crates/hardware/esp32s31/phy/src/rx/gain_calibration/quality.rs).

Nested operation durations are inclusive. These observations are not worst-case
execution bounds, RF quality or thermal qualification. Invalid/overflowed
measurements and missing active restoration fail the HIL check.

The peripheral diagnostic explicitly configures software window widening with
a 500-ppm local-clock bound through `BluetoothColdStartConfig::with_peripheral_connection`.
Cold start retains and verifies selection of the main XTAL. The bound assumes
that the board meets BLE clock requirements; it is the broadest permitted
bound, not a measurement from PHY calibration. ESP-IDF's `BT_LE_LL_SCA` default
is 60 ppm. Oscillator accuracy and connection readiness remain unqualified.
Other cold-start callers retain the default of unavailable recurring timing
until they supply their board's clock policy.

The diagnostic also supplies an explicit development Controller identity for
LL Version Exchange: Core version 5.4, unassigned company value `0xffff` and
subversion 1. Production callers supply their own identity through
`PeripheralConnectionRuntimeConfig::with_version_information`; none is
inferred from the ESP32-S31 or its PHY calibration.

## ACL calibration and terminal DTM maintenance

`cargo hil run bluetooth-peripheral-acl-backpressure` uses the `bluetooth-dtm`
image and the same kernel/BlueZ ATT fixture as the calibration workload. The
diagnostic Host negotiates MTU, then retains one RX ACL credit for a fragmented
ATT Write Command while continuing to read HCI events. The peer socket remains
open until the Host observes Disconnection Complete (`0x08`). The scenario
requires this event before credit return, with target-observed elapsed time
within 250 ms before to 1000 ms after the negotiated supervision timeout
(supported test range 100–8000 ms). These margins cover event/Host observation
timing; the production supervision deadline is unchanged.

After explicit credit return, the runner waits for the old connection's queued
data and Host credits to drain. It reconnects on the same boot and HCI session,
checks a fresh bidirectional MTU exchange and its completion credit, restores
the fixture and cold-retires the radio. `AclBackpressure` owns the diagnostic
Host selection and `HoldAclCredit` owns the retained credit. Neither operation
changes production flow control or PHY thresholds. The test establishes a
Host-backpressure timeout path; the independent abrupt RF-loss requirement
remains separate.

`cargo hil run bluetooth-peripheral-acl-calibration` uses the normal automatic
maintenance image and the kernel/BlueZ ATT fixture. It requires an initially
powered-off dedicated adapter, a public LE peer at 7.5 ms, and the four advertised
Host ACL credits. The Host exchanges MTU once, then sends four sequenced 64-byte
ACL packets per burst; the peer verifies complete notification contents and the
runner checks exact completion credits. Empty peer ACKs must allow repeated
maintenance without requiring reverse payload traffic. Scenario values specify
the observation duration and minimum number of full RX/TX calibrations.

The typed `CalibrationTraffic` operation configures real calibration thresholds
only at the idle command boundary, retains the original values, and restores them
before cold retirement. Samples and the production PHY algorithms are unchanged.
`AclBurst` rejects unreturned credits, a wrong interval or an inactive connection.
Wire protocol changes require matching firmware; archived images remain tied to
their original protocol and source identity.

`cargo hil run bluetooth-dtm-maintenance-deadline` starts real TX and RX tests
in separate captures. It accepts one autonomous reboot in an explicit time
window shorter than the diagnostic lease, verifies the platform software-reset
reason, and requires peer packet silence before sending a DUT Host Reset or Test
End. Peer measurement errors remain failures even when reset itself is observed;
both phases retain their independent results. This requires a DTM peer that
reliably completes Test End after RX with zero received packets. Packet silence
is a DTM observation, not a spectrum-wide emissions measurement or proof of
continuous packet cadence before reset.

`cargo hil run bluetooth-dtm-watchdog-reset` uses the separate
`bluetooth-watchdog-reset` image. Its DTM client acquires a deadline lease
from the SoC service, which owns TIMG1 and arms MWDT1 for ten seconds after
the first successful DTM TX/RX start. The client never completes or renews
that lease; later HCI commands do not extend it. Automatic PHY
maintenance is absent from this image. DTM continues through its normal
production runner until the hardware reset. The host requires the MWDT1 reset
reason and the same before/after peer gates as the software-reset scenario.
This is fault injection to test reset of active RF, not an implementation of
shared-PHY deadline ownership, a blocked-poll injection, or a proof that RX/DMA
is quiescent. RX silence does not prove receiver shutdown; those limits remain
distinct from the positive observation of TX packets followed by silence.
Each phase's `deadline.json` reports `reset_verified` separately from `rf`:
post-boot DTM silence, correlated TX packet cessation and peer identity.
`tx_dtm_cessation_observed` is null for RX, and `rx_dma_stop_observed` is always
null because this scenario has no such measurement. Neither field promotes a
partial result into a passing run; peer errors still fail the scenario.


The `bluetooth-peripheral-encrypted-acl` scenario uses the same production
Controller and exact ACL echo Host, with an explicitly selected diagnostic
LTK provider. The Host validates Rand/EDIV and the sole live handle, pumps HCI
while replying to the key request, and rejects plaintext application data in
its evidence. Key values are public fixture constants; session SKD/IV still
come from production hardware entropy. See the
[encrypted ACL fixture](../../host/linux-bluetooth/README.md#encrypted-peripheral-acl-fixture)
for the exchange, reconnection and cleanup gates and their limits.

The `bluetooth-peripheral-missing-refresh-key` scenario selects a one-shot
negative reply only for the replacement LTK after successful initial encryption.
It checks termination and encrypted recovery through the same production
Controller. See the [key failure fixtures](../../host/linux-bluetooth/README.md#key-failure-fixtures)
for exact peer/target criteria and helper requirements.
