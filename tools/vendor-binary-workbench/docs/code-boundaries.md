# Reviewed code boundaries

Sized ELF function symbols are authoritative analysis roots. Stripped code,
zero-sized symbols, and hand-written assembly can leave executable bytes that
do not belong to any such root. `symbols inventory` records those gaps and
emits conservative candidates only when a zero-sized text symbol or a direct
RISC-V call/tail transfer supplies concrete entry evidence.

Candidates are generated facts, not functions. The reviewed code-boundary
workspace is the explicit promotion boundary:

```text
executable sections
        ↓
symbol inventory gaps and entry evidence       generated, replaceable
        ↓
code/boundaries.toml                            reviewed, editable
        ↓
accepted physical function boundaries           trusted project input
```

IR prefix selection chooses report roots only. The resolver loads every
accepted range for the authenticated artifact so a reviewed helper with a
different name prefix can still be reached and composed like an ordinary ELF
callee.

## Project configuration

The workspace depends on the complete project symbol inventory:

```toml
[analysis.symbols]
output = "generated/findings/symbols.json"

[code]
pack = "code/boundaries.toml"

[code.review]
output = "generated/reports/code-boundaries.md"
```

Generate the inventory, then create the pack exactly once:

```console
cargo vendor-binary-workbench symbols inventory --project PATH --run-spec LOCAL_RUN
cargo vendor-binary-workbench code init-pack --project PATH
```

`init-pack` refuses to overwrite an existing pack. Regenerate generated facts,
but merge new candidates into reviewed data deliberately rather than replacing
human decisions.

If one physical artifact is bound through several source roles, the inputs
retain every `source + digest` guard but the physical candidate appears only
once. Every alias therefore consumes the same accepted bytes and name instead
of requiring contradictory duplicate review decisions.

## Review decisions

Each candidate starts as:

```toml
[[boundaries]]
source = "rom"
artifact-sha256 = "..."
section = ".text"
entry-offset = 0x12a0
end-exclusive-offset = 0x12dc
status = "unreviewed"
```

Accept a candidate by assigning a stable identifier:

```toml
status = "accepted"
name = "recovered_radio_init"
```

The reviewer may reduce `end-exclusive-offset` when the generated limit also
contains padding or data. The range may never be empty or extend past the
generated candidate limit.

Reject non-code or an unsupported candidate explicitly:

```toml
status = "rejected"
reason = "aligned literal pool, not executable code"
```

Rejected entries require a reason. Unreviewed entries cannot carry a name or
reason, and accepted names must be unique identifiers.

## Validation and review output

```console
cargo vendor-binary-workbench code validate --project PATH
cargo vendor-binary-workbench code validate --project PATH --deny-unreviewed
cargo vendor-binary-workbench code review --project PATH
cargo vendor-binary-workbench code review --project PATH --check
```

Validation fails closed when:

- the project ID differs;
- a source or artifact SHA-256 guard changed;
- a generated candidate is missing from the pack;
- a stale pack entry has no current generated candidate;
- a reviewed range exceeds its generated gap limit;
- a decision lacks its required name or reason;
- two accepted boundaries claim the same name.

The Markdown review is generated presentation. It combines each decision with
its zero-sized-symbol and direct-control-flow evidence; editing it has no
effect on analysis.

`project browse` exposes the same typed workspace in its `Code` tab. It is a
read-only inspection surface for filtering candidates and checking the exact
artifact, section, range, decision, and call/tail-call evidence before editing
the TOML pack and reloading.

## Current analysis boundary

This layer establishes reviewed physical boundaries without inventing source
semantics. Project IR, MMIO, and interface analysis consume one effective code
catalog that combines ordinary sized ELF functions with accepted reviewed
ranges. Ad-hoc commands without a project continue to see only physical ELF
symbols because they have no reviewed trust root.

Before loading an accepted range, the effective catalog authenticates the
current run-spec artifact against the pack SHA-256 guard again. The RISC-V
backend then extracts exactly the reviewed section bytes and relocations. The
reviewed name becomes the analysis identity; source, digest, member, section,
and offsets remain in the pack and generated review as its provenance.
