# HIL terminology and roles

This reference fixes one meaning for each hardware-in-the-loop term and one
owner for each role. Code, documentation and reports use these words in
these meanings; a word listed under [retired words](#retired-words) is not
used for a HIL concept. The [host architecture](../hil/host/architecture.md)
and the [protocol](../hil/protocol/README.md) define the contracts in detail.

## Hardware and the stand

| Term | Meaning |
| --- | --- |
| Chip | A silicon type, named in full (`esp32s31`, `esp32c5`). Its facts live in `platform/<chip>/chip.toml`. |
| Board | One physical board carrying one chip, identified by its MAC as `board:<MAC>`. |
| DUT | The board a scenario tests. A board is a DUT only within a run that names it so. |
| Peer | A board or radio that is the other end of a scenario: a Direct Test Mode reference board, an IEEE 802.15.4 peer board, a Wi-Fi station. A peer board is flashed and reset like a DUT. |
| Fixture | Host-controlled equipment that is not a board: the laptop radio, the OpenWrt router, the Bluetooth adapter. |
| Fixture provider | The installable host software that operates one kind of fixture (`linux-net`, `linux-bluetooth`). |
| Air | The shared radio medium, claimed as a resource like a board. |
| Stand | Everything one host operates for HIL: its boards, fixtures and air, and the arbiter that orders access to them. |
| Board support | The host operations for one chip: build inputs, flash, reset, console, JTAG and post-mortem, selected by the chip profile's boot flow. |
| Recovery | Bringing a board that does not answer back to a flashable state; a board that cannot be recovered is quarantined. |
| Quarantine | A board withheld from leases until a person or a later recovery returns it. |

## Access to the stand

| Term | Meaning |
| --- | --- |
| Owner | The agent or person a lease is charged to. |
| Claim | One resource a request needs, exclusive or shared: `board:<MAC>`, a fixture key, `air`, or `stand` (everything). |
| Ticket | A waiting request in the arbiter's queue. |
| Lease | A granted set of claims held by one owner until released or preempted. |
| Job | A detached, enqueued command (`run`, `ab`, `bisect`) that waits for its lease. |

## Firmware

| Term | Meaning |
| --- | --- |
| Image | One built firmware for one chip. |
| Image class | A named recipe for an image: its chip, runtime features and placement. Scenarios name the class they need. |
| HIL agent | The firmware services on a board that serve the HIL protocol: the console and the services an image class compiles in. |
| Image keys | The keys of every endpoint, message and property an image serves, reported by `base/image-keys`. The host derives each image class's keys from its features with the same function the image uses. |

## The protocol

| Term | Meaning |
| --- | --- |
| Message | A typed value with a path and a key; see the [protocol](../hil/protocol/README.md). |
| Module | A path prefix and its directory of messages and payloads (`base`, `wifi`, `network`, …). |
| Endpoint, topic, property | A request with one response; a device message; a marker an image advertises among its image keys. |
| Request identity | The session identifier and request identifier a request and its replies share. |
| Session | A protocol session: a host-assigned identifier that scopes requests, or a traffic session the `network` module configures. The host's record of a console stream is a capture, not a session. |

## Tests and runs

| Term | Meaning |
| --- | --- |
| Family | A radio family of workloads: `system`, `ieee80211`, `bluetooth`, `ieee802154`, `coex`. |
| Scenario | A versioned catalog entry: one workload, its configuration, image class, requirements and checks. |
| Workload | What a scenario does on the stand, identified as `<family>/<kind>`. |
| Plan | The resolved scenarios, images and claims of a selection, computed without hardware. |
| Run | One invocation's execution of a plan and its sealed directory. |
| Repetition | One execution of a scenario's workload within a run. |
| Attempt | A scenario's completed repetitions, sealed on their own so that they survive a later interruption. |
| Capture | The host's record of one boot's console stream: raw bytes, decoded messages and link health. |
| Observation | A typed fact the host records: a decoded message, a host measurement, a fixture state. |
| Measurement | A named numeric observation with a unit. |
| Check | A predicate over observations that a scenario declares. |
| Verdict | The outcome a scenario's checks decide: passed or failed. |
| Outcome | A repetition's or scenario's result: passed, failed, or broken, blocked, skipped, interrupted when no verdict applies. |
| Infrastructure failure | A failure of the stand, a fixture or the link, classified apart from a verdict. |
| Post-mortem | The evidence a failed repetition's board still holds, read without a reset. |

## Evidence and readiness

| Term | Meaning |
| --- | --- |
| Evidence | The sealed contents of runs. |
| Bundle | A run's directory with its integrity index. |
| Seal | The integrity index that fixes a bundle's or an attempt's files. |
| Observer | The build of the observation crates that produced a run, identified by its receipt. |
| Observation crates | The crates whose code shapes a recorded observation or verdict: the protocol, the link, execution, the workloads, the verdict and evidence. Stand, board support and the image builder are not among them. |
| Build record | `source-inputs.json`: everything an image's build read. |
| Closure | The input files of a run that decide whether its evidence is current. |
| Messages used | The message paths a run sent or received, sorted and unique, in its manifest's `messages_used`. |
| Shard | A tracked record in `hil/evidence/<chip>/` of observations that qualify on a checkout. |
| Qualification | The independent evaluation of readiness from sealed evidence. |
| Capability | A unit of readiness in the qualification catalog (`qualification/catalog/`). |
| Program | The capabilities a qualification target requires and its evidence policy. |
| Baseline | The source revision the user chooses to qualify. |
| Last-known pass | A pass on sources that have changed since: information, not a gap. |

## Roles and their owners

Each role has one owner. A role depends only on the roles above it in this
table; the verdict depends on no stand operation.

| Role | Decides | Owner |
| --- | --- | --- |
| Wire contract | Messages, keys, framing; the platform trace events; which keys an image built with given features serves | `oer-hil-protocol`, `oer-hil-trace`, `oer-hil-image-keys` |
| HIL agent | How a board serves the protocol | `oer-hil-target-core` and the chip runtimes under `hil/targets/` |
| Board support | How one chip is flashed, reset, observed and examined after a failure | `oer-hil-board` (the port, the ESP-IDF bootloader flow, resets), `oer-esp32s31-hil-board` (the staged flow, calibration slots), `oer-hil-runner-core` (`post_mortem`) |
| Stand | Who holds which board, fixture and air; recovery and quarantine | `oer-hil-arbiter`, `oer-hil-runner-core` (`lab`, `recovery`) and `cargo hil` |
| Fixtures | Preparation and restoration of host equipment; privileged installation | `oer-hil-fixture`, `oer-hil-fixture-install` and each family's fixture module |
| Image builder | Which bytes an image class produces and from which sources, and the firmware and build records it hands to evidence | `oer-hil-image` (`frozen` builds from a source snapshot, `record` writes the records and implements the recipe verification checks them against) and `tools/firmware` |
| Execution | How a workload drives the DUT, peers and fixtures and records observations | `oer-hil-runner`, `oer-hil-link` (the capture, protocol exchange and measurements, through its `Dut` port), `oer-hil-runner-core` (`context`) and the `oer-hil-runner-<family>` packages |
| Verdict | Which checks pass for a scenario's observations | `oer-hil-image-class` (the image a scenario needs and the keys it must serve), the scenario model and campaign plan in `oer-hil-scenario` and each family's workloads |
| Evidence | How a run is written, sealed and verified | `oer-hil-evidence`, `oer-hil-source-snapshot` (the sources a run and its images were built from), `oer-hil-durable` and `oer-hil-schema` |
| Qualification | Whether sealed evidence establishes readiness | `oer-qualification` |

## Retired words

| Word | Use instead |
| --- | --- |
| target (for a chip) | chip |
| target (for firmware on a board) | HIL agent or image |
| cell | stand, or the board and fixtures a run names |
| campaign | run |
| session (for a console stream) | capture |
| capability (for what an image serves) | image keys, or property |
| platform (for `platform/<chip>/`) | chip platform package; `platform` alone is the `package.metadata.open-radio.platform` field (`portable`, `host`, `chip`) |
