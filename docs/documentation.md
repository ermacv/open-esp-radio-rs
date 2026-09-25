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
| `docs/book.toml` and `docs/SUMMARY.md` | mdBook configuration and guide order; documents elsewhere remain with their owners |

The documentation tree follows code ownership. A subsystem's detailed contract
has one canonical location; other documents link to it. Root navigation does
not repeat a complete crate inventory or a second capability matrix.

The guides in `docs/` render as an mdBook book (see
[Build the guides and API documentation](#build-the-guides-and-api-documentation));
add a new guide to `docs/SUMMARY.md`. Links from a guide to documents or code outside `docs/`
stay relative in the source and point to the same commit on GitHub in the
rendered book. The published site adds `cargo doc` API documentation under
`api/`. Component documents are read on GitHub beside their owners.

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

Give a diagram one question to answer: dependency direction, knowledge flow,
call sequence or resource lifetime. Label what its arrows mean, keep abstraction
levels consistent, and include a text explanation. Link a worked example to
its actual source owners instead of duplicating numeric register definitions.
Keep learning routes short; long operator references can use topic pages with
stable entry anchors in their original index.

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
The sole user-authorized work-plan exception is
[Blobray completion](../tools/blobray/PLAN.md). It owns stage scope, acceptance
criteria and status, not architecture contracts or generated run reports.
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
API documentation follows docs.rs conventions: each package is documented once,
for the target and features in its `[package.metadata.docs.rs]` table. Portable
and host packages use the host; chip packages set
`default-target = "riscv32imafc-unknown-none-elf"`, because their API exists
only there. `cargo xtask doc` runs one `cargo doc --no-deps` per documentation
target with `RUSTDOCFLAGS=-D warnings`, so broken intra-doc links fail, and
`cargo test --doc --workspace` for all host doctests. Private items are
documented locally with `cargo doc --document-private-items`.
`cargo xtask check source-only` includes only static documentation checks.

These commands use locked, offline Cargo operations. Rendered catalog views live
below `target/docs/catalogs/`. The commands do not query external links, load
runtime evidence, evaluate readiness, flash a device or install fixtures.
Documentation checks establish consistency within their stated scope, not radio
behavior or qualification.

For guide changes, also run `mdbook build docs` and `mdbook test docs`. Markdown
link validation does not check Mermaid syntax; view changed diagrams in the
rendered book (`mdbook serve docs`) or on GitHub.

The checked Markdown surface consists of tracked documents plus the root
`CONTRIBUTING.md`, owner documents under `docs/`, package `README.md` files and Cargo `readme` targets,
including newly created files before they are staged. Arbitrary untracked task
notes are outside that surface. Local relative links, images, references,
directory targets, percent-encoded paths and supported anchors are checked;
external URLs are reported as `external-not-checked`.

## Build the guides and API documentation

Install the tool versions that
[the Documentation workflow](../.github/workflows/docs.yml) pins:

```console
cargo install --locked mdbook --version 0.5.4
cargo install --locked mdbook-mermaid --version 0.17.1
cargo install --locked mdbook-permalinks --version 3.0.2
```

From the repository root:

```console
mdbook-mermaid install docs
mdbook build docs
mdbook test docs
mdbook serve docs
cargo xtask doc
```

`mdbook-mermaid install docs` writes the Mermaid scripts that `docs/book.toml`
loads; Git ignores them. `mdbook build docs` writes `target/book/`. The
[mdbook-permalinks](https://docs.tonywu.dev/mdbookkit/permalinks/) preprocessor
rewrites a guide's relative link to a file outside `docs/` into a GitHub link at
the commit being built; links inside the book are unchanged.
`cargo xtask doc` writes the standard Cargo documentation directories:
`target/doc/` for host packages, `target/riscv32imafc-unknown-none-elf/doc/` for
chip packages and the Wi-Fi composition workspace's own target directory.

Pushes and pull requests run the workflow's checks and package one site: the
book at the root and API documentation under `api/host/`, `api/esp32s31/` and
`api/esp32s31-wifi/`. A manual run from `main` deploys it to the
[Pages address](https://ermacv.github.io/open-esp-radio-rs/) with the official
GitHub Pages actions.

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
- [Write the Docs](https://www.writethedocs.org/guide/writing/beginners-guide-to-docs/)
  connects project purpose, a small example, installation and contribution paths.
- [C4's introduction](https://c4model.com/introduction) explains why diagrams
  need consistent abstraction levels and clear relationships. Here Mermaid
  presents selected views without imposing a new architecture vocabulary.
