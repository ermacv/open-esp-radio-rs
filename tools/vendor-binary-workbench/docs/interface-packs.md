# Reviewed interface and semantic packs

The interface workspace joins three deliberately separate inputs:

- generated JSON facts from `interfaces discover`;
- a project-specific reviewed interface pack containing table layouts and ABI;
- reusable semantic catalogs containing operations such as RTOS notification,
  NVS access, logging, and blocking delay.

Neither a symbol name nor a slot offset assigns semantics on its own. A
binding exists only after `interfaces validate` accepts all three layers.

## Project configuration

Configure paths relative to `vendor-project.toml`:

```toml
[interfaces]
facts = "generated/findings/interfaces.json"
pack = "interfaces/reviewed.toml"
```

Attach reusable operation vocabularies through a
[platform pack](platform-packs.md). The shipped
[`embedded-semantics.toml`](../catalogs/embedded-semantics.toml) catalog is a
starting vocabulary selected by such a pack, not a platform implementation.
The platform pack supplies vocabulary and an optional harness; only the
reviewed interface pack may bind a concrete observed slot to an operation.
`semantic-catalogs` under `[interfaces]` is rejected.

## Bootstrap and daily workflow

Generate facts from local vendor inputs:

```console
cargo vendor-binary-workbench interfaces discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.run
```

Create a pack once:

```console
cargo vendor-binary-workbench interfaces init-pack \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

`init-pack` creates exact, `status = "unreviewed"` anchors and slots for the
current facts. It includes the artifact SHA-256 emitted by discovery and
refuses to overwrite an existing pack. `--output PATH` creates a separate
draft.

If one physical artifact was deliberately bound to several logical source
IDs, initialize from a `interfaces discover --source ID` report. The template
generator refuses to choose one identity arbitrarily.

After editing, validate the complete workspace:

```console
cargo vendor-binary-workbench interfaces validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

The command reports every resolved reviewed binding with its layout version,
ABI, semantic operation, vendor functions, concrete call-site addresses, call
kind, and recovered argument expressions. The default view is human-oriented;
`--format tsv` uses `INTERFACE-BINDING` plus following `INTERFACE-CALL` rows,
while JSON/JSONL emits one `interface-workspace` record with nested bindings,
calls and typed arguments. `interfaces init-pack` similarly emits one
`interface-pack` result.
Use `--deny-unreviewed` in CI to return a non-success status while any observed
slot or declared anchor remains unreviewed. That policy failure is represented
as `status = "unreviewed"`, not as a structurally valid result.

Regenerating facts never modifies the pack. Validation reports stale slots,
stale artifact guards, ambiguous selectors, and new unreviewed observations.

## Anchor and layout format

An anchor selects observed evidence by logical source, pointer root, and
container load path. It deliberately does not use call-site addresses as
stable review keys, but validation retains every current call site matched by
the selected table and slot:

```toml
schema = 1
id = "esp32s31-radio-rev0"
calling-convention = "riscv-ilp32"

[[anchors]]
id = "wifi-osi-v9"
status = "reviewed"
origin = "observed"
source = "libpp"
root-kind = "relocated-symbol"
symbol = "g_osi_funcs_p"
addend = 0
addressing = "absolute"
container-path = [{ offset = 0, width = 32 }]
layout-version = "vendor-v9"
pointer-width = 32
layout-size = 0x1d8
slot-stride = 4

[[anchors.guards]]
kind = "artifact-sha256"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
```

`member = "file.o"` may be added for an exact archive-member match. Omitting
it intentionally merges matching use from several members. Validation rejects
overlapping anchors, so generalizing a generated anchor requires removing the
now-redundant exact anchors.

Other root kinds are `absolute-address` with `address`, and
`function-argument` with `argument`. Symbol addressing is one of `absolute`,
`pc-relative`, or `got`. Signed offsets and load widths in `container-path`
must match discovery exactly.

A reviewed layout requires a non-placeholder version, pointer width, byte
size, slot stride, and at least one guard. Slot offsets must be non-negative,
stride-aligned, pointer-sized, and contained in `layout-size`.

## Version, magic, and size guards

An artifact digest is a strong static revision guard. When a table contains a
runtime header, use an explicit value guard instead or in addition:

```toml
[[anchors.guards]]
kind = "runtime-value"
purpose = "version"
offset = 0
width = 32
mask = 0xffffffff
value = 9
```

`purpose` is a stable descriptive ID such as `version`, `magic`, `size`, or
`capabilities`. The workbench checks range, mask, value, and layout bounds.
This records a required runtime precondition; it does not claim that vendor
code actually checked the field. Later IR/adapter generation must preserve or
materialize that guard before relying on the layout.

Facts generated before artifact hashes were added cannot satisfy a digest
guard. Regenerate them instead of silently dropping the guard.

## Slots and ABI

Observed slots begin unreviewed. A reviewed slot names its ABI explicitly:

```toml
[[anchors.slots]]
offset = 0x38
width = 32
status = "reviewed"
origin = "observed"
name = "queue_send_from_isr"
arguments = ["opaque-handle", "const-ptr", "out-ptr"]
return = "bool"
semantic = "rtos.queue.send-from-isr"
```

Supported scalar ABI types are `bool`, `i8`/`u8`, `i16`/`u16`, `i32`/`u32`,
`isize`/`usize`, `ptr`, `const-ptr`, `mut-ptr`, `out-ptr`, `fn-ptr`, and
`opaque-handle`; `void` is additionally valid as a return. `variadic = true`
marks a variadic tail. The pack-level calling convention must match the
project target.

Slot status and origin have independent meanings:

| Field | Meaning |
| --- | --- |
| `status = "unreviewed"` | Observation is preserved without a name, ABI, or semantic claim |
| `status = "reviewed"` | Name and complete ABI are required; semantic binding is optional |
| `status = "ignored"` | Observation is a reviewed false positive; metadata is forbidden |
| `origin = "observed"` | Exact `(offset, width)` must exist in current facts |
| `origin = "manual"` | Slot must be absent from facts and is an explicit human addition |

A manual anchor is also allowed for a documented table not reached by current
code, but it must be reviewed. Manual entries are kept visibly separate from
machine observations.

## Semantic catalogs

A catalog defines reusable behavior without mentioning a chip, symbol, table,
or slot:

```toml
schema = 1
id = "embedded-platform"

[[operations]]
id = "rtos.queue.send-from-isr"
domain = "rtos"
summary = "Attempt to enqueue from interrupt context"
argument-roles = ["queue", "item", "task-woken-out"]
return-role = "success"
effects = ["scheduler.queue-send", "scheduler.may-wake-task"]
replacement = "async.channel.try-send"
```

The slot argument count, variadic flag, and void/non-void return must agree
with the referenced operation. Unknown operations fail validation instead of
acquiring behavior from a familiar function name. `replacement` is a design
hint, not executable Rust and not an equivalence claim.

The shipped catalog covers representative RTOS, NVS, logging, and delay
operations. A project can add narrower operations when argument roles or
effects differ. For example, a blocking vendor delay and a counted busy loop
remain distinct operations even if both may eventually become an async timer.

## Trust boundary

The validated workspace establishes that reviewed metadata is internally
consistent and still attached to current evidence. For observed slots it also
proves a structural join from artifact/root/container/slot to each reported
static call instruction and preserves the recovered register arguments. It
does not prove that a branch reaches that instruction at runtime, the order of
calls, a C prototype, runtime table contents, callee behavior, storage
durability, or Rust equivalence. Those claims belong to higher-level IR,
effect contracts, and execution profiles. If an independent target-harness
contract supplies a semantic ID for the same exact caller/site, function
review fails on disagreement with the interface pack instead of choosing one.

This separation keeps the generic backend useful for any RV32 vendor artifact:

```text
RV32 ELF/archive -> generated interface facts
                              + project interface pack
                              + reusable semantic catalogs
                                         |
                                         v
                      resolved reviewed bindings + call sites
                                         |
                                         v
                           future IR/effect/adapter validation
```
