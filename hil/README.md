# Hardware-in-the-loop infrastructure

HIL runs compiled production code on an attached device and records typed,
immutable evidence bundles. It does not implement radio behavior, and a passed
scenario or internally consistent bundle is not by itself a qualification
decision.

## Choose a route

| Task | Start here | Hardware or host effects |
| --- | --- | --- |
| Understand execution, cleanup and bundle integrity | [Execution and evidence architecture](host/architecture.md) | Reading only |
| Inspect scenario definitions | [Scenario catalog](scenarios/README.md) | Reading only |
| Configure the lab and run commands | [Host setup and operations](host/README.md) | The guide labels build, fixture and DUT effects |
| Preview or install Linux fixture software | [Canonical fixture installation](host/README.md#linux-fixture-software-installation) | Dry-run is offline; installation uses interactive privilege but no RF |
| Build or understand target firmware | [ESP32-S31 target](targets/esp32s31/README.md) | Build-only sections are separate from flash/run sections |
| Install or operate the Linux network fixture | [`linux-net`](host/linux-net/) and its checked helper interface | Requires explicit host setup and, for installation, privileges |
| Install or operate the Bluetooth fixture | [Linux Bluetooth setup](host/linux-bluetooth/README.md) | May reset or reconfigure the selected adapter |
| Evaluate readiness | [Qualification](../qualification/README.md) | Reads declared programs and eligible sealed evidence independently |

The wire contract lives in `protocol/`, versioned scenarios in `scenarios/`,
host orchestration in `host/runner/`, and embedded consumers in `targets/`.
Chip-independent target logic and its host tests live in
[`target-core/`](target-core/README.md).
The evidence contract shared by the runner and the qualification evaluator —
observer build identity, Cargo input projection and canonical scenarios —
lives in the `schema/` crate; its `producer` feature adds the Cargo-running
operations that only the runner and repository tools use.
Generated runs stay below `target/hil/<chip>/runs`; they are not tracked.

## Safe source-only route

From the repository root, these commands do not open a serial device, change a
fixture, flash a DUT, or transmit RF:

```console
cargo xtask check docs --list
cargo hil scenario list
cargo hil scenario validate
cargo hil fixture probe-plan
cargo hil report verify [run-id]
```

The final command reads existing local evidence and can fail when no requested
bundle exists or a seal is inconsistent. `cargo hil image build <image-class>`
is build-only, but it executes the embedded build/tooling pipeline and writes
artifacts. `doctor` inspects machine and fixture prerequisites; it is not a
source-only check and does not prove that the current fixture configuration is
ready for a scenario.

## Hardware route

Copy `hil/local.example.toml` to ignored, mode-0600 `hil/local.toml`, configure
the stable cell/DUT identities and required fixture values, install only the
helpers required by the selected scenario, then run `cargo hil doctor` and
`cargo hil plan <scenario-id>`. Flash, replay, fixture-check and run commands
can reset or modify the named DUT/fixture and require their exclusive leases;
follow the [host guide](host/README.md) for their exact prerequisites and
effects. “Without a DUT” does not mean “without host or peer-adapter changes.”

## Run and evidence lifecycle

The runner resolves the catalog plan, acquires cooperative cell/serial/fixture
leases, and captures secret-free lab provenance while the fixture lock is held.
It then builds or imports firmware into the new run directory and flashes the
archived `application.bin`, not an unrecorded build output. Each repetition
owns its process, UART and fixture resources; partial observations survive
ordinary errors and cancellation.

Cleanup runs before repetition attachments and its result is stored separately
from the workload failure. A cleanup failure can make the repetition broken
and quarantine later network work; it cannot turn the workload into a pass.
Each completed scenario repetition set publishes its own immutable attempt seal.
Those observations remain available if a later scenario or the campaign is
interrupted. After scenario results are written, the runner writes suite/JUnit/HTML records,
marks the manifest complete and finally creates `integrity.json`. Interrupted
unwinding instead records an interrupted manifest and attempts its own seal.
Abrupt process termination can leave the active scenario without a seal;
previously published scenario seals remain usable.

Qualification independently checks the selected program, source/build binding,
completion seals and repetition requirements. A verified snapshot matching the
complete current inputs supplies direct evidence, including dirty inputs. Property-scoped
[engineering reviews](../qualification/evidence-reviews.md) can establish
applicability to another build. History pages and Markdown
remain navigation and presentation, never proof input.
