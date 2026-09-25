# ELF memory report

`oer-memory-report` is a target-neutral, read-only ELF analyzer. It
keeps two kinds of evidence separate:

- the ELF is authoritative for addresses, section sizes and symbol sizes;
- a project-owned TOML policy is authoritative for ownership, placement
  requirements, reasons and optimization notes.

It never infers that an allocation is DMA-safe merely from a familiar name.
Required policy rules fail closed when their symbol disappears or moves to the
wrong region.

```console
cargo memory report \
  --elf /absolute/path/to/runtime.elf \
  --policy hil/targets/esp32s31/memory/tcp.toml

cargo memory audit --elf ELF --policy POLICY
cargo memory stack --elf ELF --policy hil/targets/esp32s31/stack.toml
cargo memory code --elf ELF
cargo memory code-diff --before OLD.ELF --after NEW.ELF
cargo memory mono --input CRATE.mono_items.json
cargo memory diff \
  --before OLD.ELF --after NEW.ELF --policy POLICY
```

Select the retained `runtime.elf` from the completed HIL image or standalone
firmware bundle reported by its builder. Image/network selections and build IDs
change output paths; an old Cargo cache path does not identify the selected
firmware. The policy must match that image's composition.
HIL images use `hil/targets/esp32s31/stack.toml`; standalone examples use
`platform/esp32s31/stack.toml`. The HIL policy also reviews harness-owned frames.

Every command supports `--format human|json`. `stdout` contains only the
selected report; errors are written to `stderr`.

`stack` requires nonempty compiler-emitted `.stack_sizes` metadata. A missing or
empty section fails instead of claiming a successful audit without measured frames.
Report schema 2 separately inventories defined text symbols by linked address,
counting aliases once. It reports measured and unmeasured groups, Rust-mangled
and other linked text, absolute declarations (such as ROM functions), other
text definitions, and metadata entries without a matching text symbol. Linked
coverage is `complete`, `incomplete`, or `unavailable` when there are no linked
text symbols. A zero-byte frame counts as measured. Absolute declarations do
not contribute to linked-code coverage.

This is a symbol inventory, not proof that every compiled function has metadata:
stripped symbols and inlined code are outside that inventory. Rust names can
also identify naked assembly functions; other text symbols can identify vector
tables rather than callable functions. Missing entries remain visible without
automatic exemptions or inferred zero-byte frames. When a stack policy specifies
`coverage_policy`, every unmeasured linked symbol must have exactly one explicit
review; unexplained symbols fail the audit. Both ESP32-S31 policies use the
shared platform review. Reviews classify assembly, vector data and pinned
compiler runtime without inventing frame sizes. Without that policy, the exit
status checks measured budgets only; `coverage_reviewed` distinguishes scopes.
Target policy schema 4 owns separate task and dedicated-IRQ runtime headroom
reserves, a review threshold, a hard per-frame limit, the compiler move limit and runtime
headroom. Generated async `poll` functions are included. Local frames do not
prove indirect call chains, so HIL stack painting remains independently
mandatory.

A reviewed frame can name an `execution_stack` with a `storage_symbol` and
`minimum_free_bytes`. Its additional limit is the linked ELF symbol's size
minus that reserve, even below the ordinary review threshold. Missing or
unsized storage fails the audit when the matching function is present. This
prevents a primary-core allowance from admitting a frame larger than the
secondary core's stack; it does not prove the aggregate call-chain bound.

Every frame above the review threshold must match a `reviewed_frames` policy
entry and stay below its individual ceiling. A new large frame or growth of a
reviewed frame therefore fails the build instead of producing an unactioned
warning. Multiple matching rules fail with their selectors and the frame
identity, including below the review threshold: policy order must not choose
an allowance or execution stack. A rule may match multiple monomorphizations;
an unused rule does not by itself fail an image with a different composition.
The report lists the actual measured addresses matched by every rule, including
unused rules, small frames and all sides of an ambiguous match.
An optional positive `max_matches` bounds distinct measured addresses for a
reviewed selector, including small frames. Use it only where the review asserts
that cardinality (for example one concrete task entry). Aliases count once;
zero matches remain valid when an image omits the component.

The analyzer reports linker-region capacity, allocated sections, explicit
reservations, genuinely unassigned address space, policy-attributed consumers
and the largest unclassified symbols. `diff` compares both region totals and
semantic consumers, which makes buffer changes visible even when mangled Rust
symbols change between builds.

`code` inventories surviving text-symbol ranges in the supplied linked image.
Aliases and overlapping ranges count once; padding and unsized/stripped code
remain unattributed. `code-diff` compares exact text-section totals and retains
before/after address ranges for each complete alias-set identity. Duplicate
names retain all ranges; changed alias sets appear as removed/added rather than
being heuristically merged. Symbol-range deltas are not additive when symbols
overlap, and hash/name changes can prevent matching across builds. Section
deltas, not these diagnostic identities, establish the total image change.
`mono` reads one compiler-generated JSON file produced with
`-Zdump-mono-stats=DIR -Zdump-mono-stats-format=json` on the pinned toolchain.
Its counts and estimates are not linked bytes. Keep build provenance with the
compiler output; neither command rebuilds firmware or adds a mandatory gate.
The pinned compiler can emit a zero-byte file for crates without mono items.
Reports preserve this as `compiler_output_empty`, distinct from an empty JSON
array; malformed nonempty JSON remains an error. Capture retains every file
identity and requires at least one reported definition overall. This is not a
proof of compiler metadata coverage.

For a complete compiler capture through the HIL image constructor, run manually
from a clean checkout:

```console
cargo hil image mono bluetooth-secure-gatt
```

This builds in a fresh ignored `target/hil/esp32s31/mono/` directory and never
flashes or acquires a fixture. `capture.json` records the source commit,
compiler/Cargo identities, command and flags, effective locks, input JSON hashes
and exact diagnostic ELF identity. `linked-code.json` describes that same ELF.
Compiler files remain separate; their estimates must not be added to linked
bytes. An interrupted or invalid capture retains `status = building`, not a
successful manifest. Local dependency/compiler wrappers are rejected, and the
source must remain unchanged until completion. This is an explicit diagnostic
build, not qualified production timing evidence or a mandatory checkpoint.
