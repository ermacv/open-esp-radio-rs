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
| Chip `FEATURES.md` | Current implementation coverage and explicit unsupported hardware/protocol behavior |
| `examples/` | Buildable application examples and their board/configuration requirements |
| `qualification/targets/` | Machine-readable capability requirements and declared proof states |
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

A feature matrix names the chip, protocol/role, implemented boundary and
limitations. Distinguish a pure model, an executable hardware operation, a
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
Run the applicable Cargo doc-tests, rustdoc checks and command help; build a
hardware example for its declared target. Network access, flashing and fixture
installation are not prerequisites for reviewing a reference page.

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
