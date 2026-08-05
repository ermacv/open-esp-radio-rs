# Interface and trampoline discovery

`interfaces discover` inventories recoverable indirect calls before any chip,
RTOS, NVS, logging, or delay semantics are selected. It is the bridge between
raw symbols/instructions and a reviewed interface pack.

## Project use

Bind archives and ELF images in a local run spec:

```text
schema 1
input source-inventory:libpp /private/vendor/libpp.a
input source-artifact:vendor-linked /private/vendor/vendor-linked.elf
input source-artifact:rom /private/vendor/rom.elf
```

Then run:

```console
cargo vendor-code-validator interfaces discover \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --run-spec /path/to/local.run \
  --json-report generated/findings/interfaces.json
```

All artifact/inventory roles are scanned by default. Use repeatable
`--source ID` filters to select logical sources, `--name-prefix PREFIX` to
limit function names, and `--tables-only` to omit direct function pointers
that have no preceding load.

A project can make the report path reproducible:

```toml
[interfaces]
facts = "generated/findings/interfaces.json"
pack = "interfaces/reviewed.toml"
semantic-catalogs = ["interfaces/embedded-semantics.toml"]
```

With this table, `--json-report` is optional. The report is generated facts;
do not edit it.
See [reviewed interface and semantic packs](interface-packs.md) for the
separate `init-pack` and `validate` lifecycle.

## Recovered evidence

The RV32 backend propagates three kinds of pointer roots through the function
control-flow graph:

- a symbol named by a data relocation in an archive/object;
- an absolute address reconstructed in a fully linked ELF;
- an ILP32 function argument (`arg0` through `arg7`).

Each load appends its instruction address, signed byte offset, and width. A
non-return `JALR` whose base still has pointer provenance becomes an
`INTERFACE_CALL`. The report retains:

- artifact, archive member, owning function and call-site address;
- call, tail-jump, or unusual linked-jump shape;
- complete recovered root/load chain and the separate `JALR` offset;
- the last load as a slot candidate and earlier loads as the container path;
- recoverable `a0..a7` pointer/constant provenance at the call;
- matching symbol facts and association-only cross-artifact candidates.

Archive/object roots distinguish three addressing forms. Absolute
`HI20/LO12` pairs directly name the symbol. For
`PCREL_HI20/PCREL_LO12`, the ELF LO relocation names the local label at the
HI instruction; the loader resolves that pair and records the HI relocation's
actual target symbol. `GOT_HI20/PCREL_LO12_I` is kept separate: its load
resolves the symbol address from the GOT and is not counted as a table or
pointer-cell dereference. An unpaired PC-relative LO relocation is rejected as
malformed evidence rather than attributed to its temporary label.
The pairing and GOT distinction follow the
[RISC-V psABI relocation definitions](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/).

For example, this machine-code shape:

```text
relocation(g_services) -> load32(+0x0) -> load32(+0x10) -> jalr
```

is reported as a root symbol, one container dereference and slot `+0x10`.
The tool does not decide whether `g_services` is a pointer cell, whether the
slot belongs to an RTOS API, or which C prototype it has.

The JSON also groups calls with the same artifact, root and container path as
`table_candidates`, collecting observed slots and calling functions. This is
an observed-use inventory, not a declaration of table size: unobserved slots
are absent.

## Linkage boundary

Relocations are the strongest archive-level evidence for an externally named
global. The associated symbol fact says whether that name is local, exported,
undefined, common, or absolute. Definitions in another archive member or
project input are listed as navigation candidates only. Static-archive member
selection, weak precedence, linker-script rules and interposition are not
reimplemented by the validator.

For a final linked ELF, absolute roots can be associated with symbols at the
same address. The linked image remains the authoritative input when exact
selection and addresses matter. Stripped code without sized function symbols
is not currently assigned function boundaries by this command.

## Generic facts versus reviewed packs

There are three deliberately separate layers:

| Layer | Owns | Must not own |
| --- | --- | --- |
| Generic discovery | root, load chain, offsets, call site, raw ABI value flow, evidence limitations | RTOS/NVS names, C types, effects |
| Interface pack | table anchor rules, indirection depth, version/magic/size guards, slot names and ABI signatures | scheduler/storage behavior inferred only from a name |
| Semantic pack | reusable operations and effects such as `rtos.event.post`, `storage.nvs.read`, `time.blocking-delay`, replacement hints | chip addresses and unguarded table layouts |

A chip/project pack should instantiate an interface layout and refer to a
reusable semantic operation. For example, the chip-specific fact “version 9,
slot `0x38`, arguments `(queue, item, woken)`” may map to the generic operation
`rtos.queue.send-from-isr`. An async-Rust adapter can then propose a signal or
channel replacement while the original low-level call evidence remains
visible.

The implemented reviewed pack uses stable keys based on source, root selector,
container path, slot offset and a layout/version guard—not instruction
addresses. Regeneration distinguishes unchanged, new, missing, and ambiguous
slots without overwriting user decisions.

## Explicit limitations

The command sets `semantic_claim`, `table_layout_claim`,
`linker_resolution_claim`, and `completeness_claim` to `false`.

Current recovery is intentionally conservative:

- only named non-empty text symbols define functions;
- conflicting provenance at a control-flow merge becomes unknown;
- computed/indexed slot offsets are not converted into fixed slots;
- scalar expressions and stack-passed call arguments are not yet rendered;
- linker-relaxed `gp`-relative symbol accesses and other computed symbol
  addresses do not yet recover a relocation root;
- an indirect tail jump may be a dispatch edge rather than a function call and
  remains labeled as a candidate.

These limitations reduce findings; they never authorize a semantic label or a
validation claim. Reviewed reference/effect contracts remain the fail-closed
layer used to compare Rust behavior.
