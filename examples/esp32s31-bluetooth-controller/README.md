# ESP32-S31 Bluetooth Controller

This target composes the production ESP32-S31 Bluetooth Controller exactly as
an application must own it:

- the ESP-HAL platform singletons and the restricted radio PAC are each taken
  once;
- cold start claims the static BLE-PHY and DTM graphs;
- the sole hardware runner owns command, timer, and interrupt progress;
- a standard `bt-hci::Controller` read loop is polled concurrently with typed
  Host commands.

The default board smoke sequence uses only the upstream `bt-hci` 0.10.1 typed
command API. It performs:

1. HCI Reset;
2. LE Receiver Test v2 on test channel 0, LE 1M, standard modulation index;
3. a bounded one-second receive dwell followed by LE Test End;
4. LE Transmitter Test v2 on the same channel and PHY with a 37-byte PRBS9
   payload;
5. a bounded one-second transmit dwell followed by LE Test End.

The receive Test End packet count is printed as an observation; it may be zero
when no peer transmitter is present. The transmit Test End packet count is
required to be zero, as defined by HCI, and any other value fails the smoke
sequence. Command execution, the Host read pump and the hardware runner remain
concurrently polled, so command responses can make progress. The example uses
`Controller::alloc_buf` for the read side and does not encode raw HCI opcodes or
implement a local HCI codec.

Every command prints a `submitted` marker before it crosses HCI and a
`running` or `complete` marker after the typed response. A two-second command
timeout turns a lost Controller response into a bounded failure, so a board run
can distinguish command-intake, event-start and Test End liveness failures.
Before commands, `application entered`, `executor starting`, and
`Bluetooth Controller cold start submitted` markers distinguish application,
executor and radio startup; `Bluetooth Controller ready` closes cold start.

Cold-start failures distinguish common-PHY power/readback checkpoints from
registration and calibration errors. Both retain the powered radio owner and
prevent HCI startup. See the [Bluetooth capability inventory](../../crates/hardware/esp32s31/driver/bluetooth/FEATURES.md)
for the remaining maintenance and shutdown boundaries.

This is a board smoke sequence, not recorded HIL evidence. Meaningful RF
validation still requires a suitable peer or tester, controlled RF conditions
and the repository's HIL evidence process. The
[Bluetooth HIL scenario](../../hil/host/linux-bluetooth/README.md) provides
host-controlled DTM windows, independent receiver counts and silence controls.

Build and flash the complete application from the repository root:

```console
cargo xtask build firmware bluetooth-controller
cargo xtask build firmware bluetooth-controller --flash --monitor --port /dev/ttyACM0
cargo xtask build firmware bluetooth-controller --features advertising-smoke
cargo xtask build firmware bluetooth-controller --features trouble-gatt
cargo xtask build firmware bluetooth-controller --features trouble-secure-gatt
```

The [shared platform](../../platform/esp32s31/README.md) initializes PSRAM,
relocates the application and supplies SRAM interrupt stacks. Image and stack
audits run before packaging or flashing. A plain `cargo build` inside this
workspace produces the stage-two ELF, which requires the shared bootstrap.

`advertising-smoke` replaces the DTM commands. After initial Reset, it requests
nonconnectable `ADV_NONCONN_IND`, then connectable `ADV_IND`, each with a static
random address, 100 ms intervals and the local
name `open-radio`. Each case configures address/parameters/data, enables for one
second, disables, re-enables for one second, resets while enabled, reconfigures,
enables for one second and finally disables. Run without a connecting peer:
accepted connections exercise a separate, incomplete peripheral lifecycle.

The nonconnectable case exercises all three primary channels. The connectable
case selects channel 37 to fit the S31 backend's
[single-channel connectable boundary](../../crates/hardware/esp32s31/driver/bluetooth/FEATURES.md#legacy-advertising-and-scanning).
The application library owns these typed parameter commands; host tests check
that each smoke command fits its role's channel capacity.

Commands print `advertising <command> submitted` and `complete` markers, with
the command name in failures and two-second timeouts. Case and dwell markers
identify the last reached step. These observations establish HCI lifecycle
progress only: a successful Enable response or elapsed dwell does not prove
scheduler RUN, repeated events or RF transmission. Use scheduler evidence and a
BLE observer for those claims. Add `--flash --monitor --port /dev/ttyACM0` to the `xtask` advertising command
to flash this variant and open the monitor.

`trouble-gatt` replaces the direct command/read loops with the pinned
[OER Trouble fork](https://github.com/ermacv/trouble/tree/oer/numeric-comparison),
based on `trouble-host` 0.8.0. Manifests pin an exact commit, not the branch tip.
The fork adds an opt-in `PairingPolicy::NumericComparisonOnly` for new pairing;
the plaintext profile does not enable it or claim security qualification.
The application consumes the production
`BluetoothSystem` through `into_trouble`, then polls the Trouble runner and the
exact hardware runner concurrently. It repeatedly advertises as
`open-radio-gatt`, accepts one connection and exposes service `0xfff0` with the
one-byte read/write characteristic `0xfff1`. Accepted writes are printed and a
disconnect returns to advertising without reconstructing the Controller.

The Trouble Host owns bounded static resources for one connection and three
L2CAP channels. Its legacy connectable advertisement explicitly selects
channel 37 to match the current response-capable radio graph. The example
configures a 500-ppm software widening bound and an explicit unassigned
development Version Information identity; these are application policy, not
measurements or assigned product identity. `trouble-gatt` and
`advertising-smoke` are mutually exclusive because each consumes the sole Host
side of the HCI transport.

`src/gatt.rs` is the actual application used by both this binary and the
separate `bluetooth-gatt` HIL image. Its observer receives values only and never
owns HCI or supplies ATT replies. The library-only `gatt-application` feature
excludes standalone `firmware` dependencies, linker setup and panic/logging
handlers; HIL supplies its own board entry. Host tests exercise the actual attribute
table and fixed advertising payload. Run `cargo hil run bluetooth-trouble-gatt`
from the repository root to exercise Linux ATT discovery, read/write and three
graceful disconnect/reconnect cycles without reconstructing the Controller.
The value survives these connections. HIL records task/IRQ stack headroom and
correlates application observations with independent ATT replies.

This profile is explicitly plaintext. It does not establish pairing, protected
ATT access, notifications, persistent bonds or coordinated Host/Controller
shutdown. Those properties must not be inferred from a successful baseline run.

Both Trouble entry points retain a caller-owned SoC entropy service and install
its `BluetoothEntropy` binding before polling the Host. Standard HCI LE Rand
then supplies the security-enabled Host's seed without a radio-to-RNG dependency
or another Trouble extension. Enabling the Host's security feature alone does
not make this plaintext attribute table protected.

## Authenticated GATT profile

`trouble-secure-gatt` composes `security::gatt::run`, an exclusively owned USB
console and the same Host/Controller runners. The library-only
`secure-gatt-application` feature exposes that application without board entry.
It requires LE Secure Connections Numeric Comparison with `DisplayYesNo`;
Just Works, passkey entry, OOB and legacy pairing are not fallbacks. The pinned
Host rejects SC peers offering keys shorter than 128 bits.

The caller owns an asynchronous `BondStore`, bounded `RamBondStore`, and affine
Numeric Comparison requests independently of the Host. Store insertion rejects
unauthenticated records, duplicates and capacity exhaustion without evicting
existing keys. The standalone profile reserves one bond slot. RAM records live
outside Host resources but disappear on reset
or power loss; no persistent backend is implemented. Importing authenticated
metadata does not prove the original pairing method: imported records must
come from trusted Numeric Comparison enrollment. Bond changes work without
Controller privacy; the fork issues resolving-list commands only when privacy
is explicitly enabled. This profile does not enable address privacy.

Connect a peer capable of Numeric Comparison to `open-radio-gatt` and request
pairing. Compare the six-digit numbers on **both** devices. The USB console
prints a challenge and this reply format (replace all three identifiers with
the displayed values):

```text
confirm <16-hex-boot> <request-id> <six-digit-number> yes
confirm <16-hex-boot> <request-id> <six-digit-number> no
```

Only an explicit matching `yes` confirms locally; the peer must also confirm.
Wrong/stale boot IDs, request IDs and numbers are rejected. Dropping a request
invalidates queued replies; transport loss never confirms it. Ordinary firmware
logging is disabled while the console exclusively owns USB. Console observations
may coalesce and are diagnostics, not a lossless HIL evidence stream.

The service/characteristic UUIDs remain `0xfff0`/`0xfff1`. Value reads/writes and
CCCD writes require authenticated encryption. Application authorization also
requires successful bond-store insertion; restoring a connection requires the
matching stored bond and authenticated encryption. Compound ATT value reads
cannot bypass this gate. Subscribed writes queue notifications containing the
new one-byte value; queueing is not proof of peer reception. Discovery remains
public, the value survives reconnects and subscriptions are connection-local.

A full store accepts known peers only. Lost keys, failed pairing or rejected
confirmation disconnect without silent re-enrollment or key replacement. Store
errors stop the application; the outer composition keeps Host and hardware
polling so normal disconnect/advertising cancellation can progress. Restarting
the application requires a fresh Host restored from the retained store, since
a cancelled insert may already have committed. The standalone console has no
Controller-restart or bond-deletion command;
power cycling clears this RAM-only standalone profile.

The reusable `security::epoch::run` owns one Host epoch and borrows the
application's store and comparison sequence. On a stop request or failure it
drops all application/Host producers, obtains the Controller through the fork's
`Stack::into_controller`, and awaits HCI Reset while draining old events. Its
result distinguishes a requested stop, application/Host failure and Reset
failure. It is not physical retirement: the caller keeps polling hardware and
must complete timer, HCI, IRQ, platform and PHY release before cold restart.
Drive consuming transitions to completion; cancellation does not free radio
owners. The secure HIL composition exercises this sequence, reuses the same
Host storage for a fresh Host and restores the retained RAM bond. Controller
restart therefore preserves trust without new pairing; SoC reset does not.

Host tests and firmware builds do not qualify pairing or bonded reconnect over
RF. The `bluetooth-trouble-gatt` HIL scenario remains plaintext. The separate
automated `bluetooth-trouble-secure-gatt` scenario composes this secure
application with framed observations and explicit decisions after comparing
independent DUT/BlueZ numbers. It does not prove human presence or change this
standalone profile's manual consent boundary; see its
[fixture contract](../../hil/host/linux-bluetooth/README.md#secure-trouble-gatt-fixture).
Neither source scenario establishes current hardware qualification.

```console
cargo test --manifest-path examples/esp32s31-bluetooth-controller/Cargo.toml --no-default-features --features secure-gatt-application --lib
```
