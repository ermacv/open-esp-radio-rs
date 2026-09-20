# Bluetooth HIL protocol

## Trouble GATT

The `bluetooth_gatt` capability identifies the separate plaintext Trouble
application image. `QueryBluetoothGatt` returns `BluetoothGattEvidence` with
the Controller address, application connection/read/write observations and
the CPU0 stack measurement. All observations belong to the envelope's boot;
the query neither resets the Controller nor submits ATT/HCI work. Counters are
not independent proof of ATT delivery. The Linux peer validates actual replies
and reconnection. This capability does not imply pairing or secure GATT.

The separate `bluetooth_secure_gatt` capability identifies the actual secure
application with one caller-owned RAM bond slot. `QueryBluetoothSecureGatt`
reports traffic counters, the current unanswered Numeric Comparison challenge,
independent confirmation/bond/reconnect/notification counters and application
failure. It does not export keys. `ConfirmBluetoothGatt` requires the exact boot,
session zero, challenge ID and six-digit number. Stale/duplicate replies are
rejected. `BluetoothGattDecisionRecorded` acknowledges a queued UI decision,
not successful pairing or protected access. Cancellation of the application
prompt revokes even a queued answer. These commands cannot submit HCI or ATT
traffic, clear bonds or force a security transition.

`RestartBluetoothGatt { epoch }` accepts only the currently running secure
application epoch, on session zero and the discovered boot. Duplicate, stale
and stopped-application requests are rejected. Its initial snapshot acknowledges
the request, not completed retirement. Completion requires a new `epoch`,
`restarting = false`, a physical `cold_releases` increment and `old_hci_closed`:
both command and read operations on the retired handle must reject access.
The target rebuilds Host/Controller without resetting the SoC and retains only
the caller's RAM bond store and comparison history, not connections or ATT state.
Independent encrypted peer traffic without another pairing establishes key reuse;
these lifecycle counters alone do not.

`BluetoothGattResetReadGate { epoch, release }` controls a one-shot secure HIL
reader gate on the discovered boot and current epoch. Arming (`release = false`)
requires an advertising, disconnected application and does not send Reset.
An explicit restart drops the old producers and submits its real final Reset;
the wrapper suspends the sole new reader before it consumes a response.
`reset_read_gate = ReaderHeld` is the reached checkpoint, not merely an arm ACK.
Release is accepted only at that checkpoint during the same restart. It wakes
the same reader; no response is fabricated, discarded or matched to a new Reset.
The hardware runner remains polled. Cancellation does not open the gate.
The gate does not report RF cessation or model a hung silicon operation. While
held, the unchanged epoch and cold-release count must be observed; release must
be followed by actual retirement/restart and independent encrypted peer traffic.

`FailBluetoothGattResetRead { epoch }` selects failure instead of release at the
same reached checkpoint. Early, stale or duplicate requests are rejected.
`FailureRequested` acknowledges only the command; `ReadFailed` means the wrapper
actually returned its distinct injected error without reading or discarding the
real response. The epoch runner must report `InjectedReceiveFailure`, preserve
its original stop cause and retain physical execution. Release/restart are then
rejected. This tests a Host-facing I/O error, not a silicon or PHY failure; no
RF-stop, physical-close or autonomous-recovery claim follows from retention.

`FailNextBluetoothGattBondLoad { epoch }` is a one-shot secure HIL diagnostic.
It requires a connected, bonded application on the current boot/epoch, rejects
duplicates and excludes a concurrent restart. Arming it does not stop the Host.
The next actual application store load returns a backend failure without
changing the RAM record. The scenario causes that load by disconnecting its
peer. `bond_load_failures` counts consumed injections; `shutdown` reports the
redacted cause and Reset outcome from the completed Host epoch. Neither proves
physical release: `cold_releases` and `old_hci_closed` remain separate checks.
An unresolved Reset may retain physical execution. The diagnostic does not
inject silicon failure, measure RF cessation or introduce a shutdown timeout.

Secure and plaintext capabilities are mutually exclusive. Both snapshots carry
common traffic observations, which alone never establish security. Notification
counters mean Host queue acceptance; the independent peer must receive the value.

## Peripheral lifecycle

`BluetoothPeripheral::StartAdvertising` requests a bounded diagnostic
`ADV_IND` on channel 37 through production HCI and selects peer Reset, verified
peer rfkill, target Host Disconnect or target HCI Reset as the cycle
termination. Its hold is
bounded to 0..5000 ms. `Snapshot` returns boot-lifetime
advertising/peripheral RUN and disconnection counts, retries, the first terminal
reason, and separately decoded Host-side Connection Complete, Connection
Update Complete and Disconnection Complete counts. A separate counter records
Channel Map Update instants applied by the Link Layer. The connection-update counter accepts
only the recovery profile's 120-ms interval, zero latency and 2-second
supervision timeout while the sole connection is live. Successful target
Disconnect Command Status and Reset Command Complete responses have separate
counters. It also reports each
nonempty Controller-to-Host ACL fragment, each
credit returned by the target Host, each deliberately held first-fragment
credit, each fully reassembled echo accepted back from the target Host, and ACL
profile or queue faults. A distinct Host-to-Controller completion count proves
that each queued echo reached Link Layer acknowledgement. The peripheral workload declares one 27-byte Host ACL
buffer, enables Controller-to-Host flow control and uses two distinct,
sequenced deterministic 251-byte packets around a live Channel Map Update. Each packet requires
ten legacy fragments. Invalid,
out-of-order or profile-mismatched lifecycle events increment
`host_event_faults`; the last decoded disconnection reason is retained. These
are software publication observations; a correlated peer observation is
required to establish RF delivery. The start response includes the public address.
HCI rejection stages are Reset (0), base event mask (1), LE event mask (2),
Host Buffer Size (3), Controller-to-Host flow control (4), address read (5),
parameters (6), data (7) and enable (8).

`Retire` ends an admitted peripheral probe: the target completes HCI Reset with
its Host event pump running, waits for live runner handoff, then retires timer,
IRQ and HCI ownership. It extracts the shared primary/NRT register owner,
checks scheduler inactivity, empty hardware heads and absence of primary faults,
and releases Controller output before joining the exact platform reservation.
The last PHY client then closes RF, powers down temperature, resets Bluetooth
and restores retained clocks and the shared power baseline into a cold owner.
`Retired` requires `radio_cold` and separate closed-channel probes for Host
commands, Controller events and Host ACL credits. Every field must be true,
with no terminal fault, saturation or Host event/ACL fault. An incomplete
transition produces no successful retirement response. Old HCI and static ISR
storage remain closed/reserved in this terminal mode. The HIL console stays available for capability and link-health
queries; further radio operations are rejected.

`Restart` uses the same physical shutdown sequence, then reinitializes the
actual returned radio and original storage without resetting the board.
`Restarted` requires a positive cycle count, Reset through the new Host and
closed-command/event/ACL-credit probes through the old Host.

`Maintain` instead joins the idle task and retired timer with the unrouted IRQ
bank for shared-PHY tracking. HCI and the powered epoch remain intact.
`Maintained` requires a positive cycle count, a due completed tracking request,
no tracking inhibition and Reset through the same Host after resumption.
Calibration flags report the actual common/Bluetooth work selected by tracking;
they are not forced true. A not-due window cannot satisfy this diagnostic.
Both lifecycle operations use the same zero-fault gates as `Retired`; another
advertising start reinitializes the bounded Host settings after Reset.

The Bluetooth image advertises `bluetooth_peripheral` for this interface.
DTM is rejected once the peripheral probe starts. Another advertising start is
accepted only after the previous connection has reported idle restoration;
that restart omits HCI Reset. The diagnostic detail includes the cumulative
closed-connection count and last disconnect reason (including `0x3e` for
failed establishment). The host ends the probe with a
board reset; after 30 seconds the target reports lease expiry and resets the
whole board. A USB failure during the probe also resets the board. Neither
path reports logical HCI quiescence. The default connection timing policy
still lacks a local clock bound, so recurrence stops at that explicit limit.

## Encrypted ACL diagnostic

`BluetoothPeripheral::EncryptedAcl` selects the fixed-key diagnostic Host before
advertising in a fresh boot. It is exclusive with the calibration-traffic and
ACL-backpressure Hosts. `BluetoothEncryptionEvidence` reports key requests,
successful HCI key replies (including negative and deliberately wrong-key replies),
Encryption Change, Key Refresh Complete events and faults. It never carries
key bytes. The public `BLUETOOTH_TEST_*` and `BLUETOOTH_REFRESH_*` constants identify fixture material,
not a pairing or bond-storage policy. Runner and firmware must match
[`PROTOCOL_VERSION`](src/message.rs). The body bound is defined by
[`MAX_POSTCARD_BYTES`](src/framing.rs), including the largest combined
peripheral evidence record and worst-case integer encoding.

The diagnostic Host accepts the initial identity once per connection, then the
distinct refresh identity only after initial Encryption Change. During the
pending refresh it rejects application data and requires the refresh event,
not another initial Encryption Change. Disconnect clears this phase before
reconnection; counters remain cumulative for the current boot.

The optional `failure` field selects `missing-key` or `wrong-key` for the first
initial LTK request, or `missing-refresh-key` for the first replacement LTK
request after successful initial encryption. Missing keys use the standard HCI
negative reply; `wrong-key` supplies a fixed public key differing by one bit.
The refresh injection survives the initial valid reply and requires termination
with PIN or Key Missing, without successful Key Refresh Complete. After disconnect,
subsequent connections receive the correct key. The Controller and its RF/CCM
path are unchanged. Expected injections have separate counters; malformed
requests, failed HCI replies, unexpected encryption success and application
data before encryption still invalidate the scenario.

`active-data-mic` instead arms the diagnostic `rx-fault-injection` feature.
The initial key and encryption handshake remain valid. Exactly one active
encrypted data PDU has the final MIC bit flipped in its copied RX input before
the production authenticator; control, plaintext and empty packets do not
consume the request. `mic_injections` records actual corruption and
`mic_injection_armed` identifies a pending request. Disconnect disarms it;
counters remain cumulative so recovery must prove that no further injection
occurred. The stimulus is after RF reception, not a malformed over-air packet.
