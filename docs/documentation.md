# Documentation policy

Documentation describes the checked-out implementation, its supported inputs
and its limits. The owner of the code also owns its documentation. Update both
in the same change when an interface, resource lifetime, command or capability
changes.

## Where information belongs

| Location | Responsibility |
| --- | --- |
| Root `README.md` | Project purpose, implementation targets, entry points and basic checks |
| `docs/` | Contracts that cross code owners: architecture, terminology, evidence and source policy |
| Directory or crate `README.md` | That component's purpose, inputs, outputs, dependencies and operating instructions |
| Rust module and item docs | API semantics, ownership, errors, cancellation, panic and safety conditions |
| Chip `FEATURES.md` | Navigation to current implementation scope, limitations and generated capability views; not a hand-maintained readiness matrix |
| `examples/` | Buildable application examples and their board/configuration requirements |
| `qualification/catalog/` and source declarations | Canonical capability identities, dependencies and source facts |
| `qualification/targets/` | Programs that select required capabilities and evidence policy for an evaluation target |
| Owner-specific ignored output directories | Generated API docs, run reports, measurements and verification output |

The documentation tree follows code ownership. A subsystem's detailed contract
has one canonical location; other documents link to it. Root navigation does
not repeat a complete crate inventory or a second capability matrix.

## Write for a specific task

Keep instructions, reference and architectural explanation distinct. A short
README can contain each in clearly separated sections; a directory hierarchy
for every document type is unnecessary.

- An instruction names its starting directory, required tools, hardware,
  configuration and commands, followed by the expected output or next action.
- A reference defines accepted inputs, outputs, state transitions and limits.
- An architectural explanation states which owner makes each decision and
  why the dependency boundary exists.
- A tutorial uses a buildable example rather than a second implementation
  copied into prose.

Use present tense, concrete names and descriptive link labels. Keep paragraphs
short. Use tables for capability or ownership comparisons, and diagrams only
when they clarify interactions. Explain a restriction where it affects a
caller; avoid repeating the entire project's exclusions on every page.

## Document ownership and safety

For a component, identify what it owns and what the caller retains. For
hardware APIs, describe acquisition, handoff, completion and release, including
error, cancellation, timeout and quarantine paths when applicable. Put unsafe
caller obligations beside the API in a `Safety` section. Keep register field
definitions in the reviewed model/PAC instead of maintaining numeric copies
in Markdown.

Link to source or rustdoc for detailed types. Copyable examples should handle
ordinary errors. Document required target/features: a host build, a target
build and an attached-hardware run establish different things.

## Describe capability without a work log

A capability view names the chip, protocol/role, implemented boundary and
limitations. Canonical identities and declarations stay in the catalog and
source contracts; qualification programs choose requirements, and generated
views present their resolved state. `FEATURES.md` links readers to those
owners instead of copying their rows. Distinguish a pure model, an executable hardware operation, a
composed application path and independently qualified readiness. A recovered
register meaning or matching semantic leaf is not complete protocol support.

Describe an unsupported feature as a current limitation. Do not turn that row
into a task list, promised delivery order or speculative API. Throughput and
hardware readiness are derived from qualifying evidence, not remembered
measurements or a dated Markdown `PASS` table. Qualification is the readiness
authority; see [its contract](verification-and-qualification.md).

Do not track audit reports, work plans, migration histories, experiment diaries,
test-run summaries or generated inventory snapshots. Git retains source
history. Review discussions and task tracking belong outside the current
documentation; run artifacts stay in owner-specific ignored output storage.
Reviewed provenance packs, baseline identities and test fixtures used as
machine inputs are code/data contracts, not substitutes for a narrative log.

## Maintain and validate

Check relative links when moving or deleting a document. Rust `include_str!`,
Cargo readme fields, CLI help and structured evidence references can also
depend on documentation paths. Update those consumers without changing the
meaning of evidence or inventing a replacement proof.

Use the repository's pinned toolchain for examples and API documentation.
`cargo xtask check docs` checks owned local Markdown links and checks/renders
static qualification catalogs and programs. It does not build API documentation.
For API changes, use `cargo xtask check docs --package PACKAGE`: public rustdoc
and applicable host doctests run for that package's supported feature profiles.
Repeat `--package` to select more packages; `--private` includes their private API.

`cargo xtask check docs --full` checks the complete public/private rustdoc matrix,
host doctests and MCU compile-only consumers. This expensive mode also remains
part of `cargo xtask check source-only`; use it at full checkpoints, with focused
checks during local iteration. Add `--list` to any scope to inspect its plan
without executing checks. `--export-html` exports isolated rustdoc snapshots for
package/full scopes.

All modes use locked, offline Cargo operations. Their ignored reports live below
`target/docs/static/`, `target/docs/packages/` and `target/docs/gate/`, respectively,
and name their coverage explicitly. A partial run never updates the full report.
These commands do not query external links, load runtime evidence, evaluate
readiness, flash a device or install fixtures. Documentation checks establish
consistency within their stated scope, not radio behavior or qualification.

The checked Markdown surface consists of tracked documents plus owner
documents under `docs/`, package `README.md` files and Cargo `readme` targets,
including newly created files before they are staged. Arbitrary untracked task
notes are outside that surface. Local relative links, images, references,
directory targets, percent-encoded paths and supported anchors are checked;
external URLs are reported as `external-not-checked`.

## Basis for these conventions

- [Diátaxis](https://www.diataxis.fr/start-here/) distinguishes learning,
  task instructions, reference and explanation; its reference structure
  follows the system being described.
- [The rustdoc book](https://doc.rust-lang.org/rustdoc/how-to-write-documentation.html)
  describes crate introductions, public API documentation and useful examples.
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/documentation.html)
  cover examples and explicit error, panic and safety contracts.
- [Zephyr's documentation guidelines](https://docs.zephyrproject.org/latest/contribute/documentation/guidelines.html)
  describe readable heading structure and accessible tables.
- [Espressif's Rust documentation](https://docs.espressif.com/projects/rust/)
  separates learning resources from package references; device-specific API
  documentation identifies its target explicitly.
