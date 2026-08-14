# Architecture (normative)

Vendor Binary Workbench is a generic, deliberately non-inferential evidence
tool. It extracts, links, and presents observations from caller-supplied
artifacts. It may attach reviewed labels and executable boundary models, but
it must preserve uncertainty and must not promote a hint to hardware truth.

The authoritative observed source is the authenticated vendor artifact bytes
together with ABI, load mapping, and provenance. Persistent linked IR is one
canonical **derived** representation used by downstream analyses,
pseudo-Rust, and executable references. IR records instruction provenance,
uncertainty, and blockers; it is never hardware truth or an independent source
of observed behavior.

## Responsibility matrix

| Subsystem | Responsibility | Owns | Consumes | Produces | Must not know about | Source of truth | Decision |
|---|---|---|---|---|---|---|---|
| Generic Workbench | Extract, link, compare, and present facts | Analysis schemas and fail-closed algorithms | Artifact bytes, composition, add-ons | Observed facts, derived IR, pseudo-Rust, reports | Chip addresses, product policy, ledger state | Artifact bytes + provenance | Keep, simplify |
| Architecture backend | Decode/lift one ISA | ISA semantics, calling-convention implementation | Bytes, relocations, ABI | Instructions, CFG, effects, blockers | Vendors, chips, drivers, ledgers | Artifact bytes + target ABI | Keep |
| Ecosystem pack | Attach reusable vendor/RTOS vocabulary | Declarative operation names/signatures | Generic semantic vocabulary | Hints and reviewed call annotations | Chip addresses, production code | Reviewed pack | Keep, data-only |
| Chip pack/provider | Attach reusable chip facts and lifting hooks | Memory map, SVD inputs, ROM identities, chip summaries | Target ABI, ecosystem vocabulary | Chip-enriched derived analysis | Production driver, qualification policy | Reviewed chip pack; observations remain external | Keep, split |
| Project manifest | Compose one investigation | References, local workflow and output selection | Target, ecosystem packs, chip pack, reviewed workspaces | Resolved project | Reusable chip facts duplicated inline | Manifest only for composition | Simplify |
| Reviewed knowledge | Accept hardware/function meaning after review | Reviewed assertions and links to evidence | Immutable observations and external evidence | Accepted names, fields, enums, contracts | Rewriting underlying observations | Reviewed model | Keep |
| PAC generation | Publish reviewed register structure | Generator and generated-file contract | Reviewed register model | Raw PAC, bindings index | Driver/runtime policy | Reviewed register model | Keep |
| Restricted PAC/capabilities | Encode approved low-level authority | Register-local operations and capability types | Raw PAC + reviewed API policy | Non-forgeable, bounded register authority | Polling, retries, Wi-Fi roles | Reviewed API pack | Strengthen |
| HAL | Implement hardware operations and lifecycle | Sequences, waits, timeouts, recovery, serialization | Narrow capabilities | Hardware operation outcomes | Wi-Fi/BLE/SoftMAC policy | Reviewed sequences + production implementation | Strengthen incrementally |
| Driver | Implement protocol/runtime behavior | Wi-Fi/BLE/SoftMAC state and policy | HAL operations | Production behavior | Raw MMIO/PAC authority | Compiled production Rust | Keep |
| Verification models/add-on | Model environment and compare observations | Scenarios, external services, comparison relation | Compiled vendor and Rust artifacts, inputs, declarations | `MATCH`, `DIFF`, or `INCOMPLETE` evidence | Product readiness decisions | Artifacts + recorded observations | Keep, separate from knowledge |
| Dispositions | Declare reviewed mapping and claim ceiling | Vendor-to-production binding declarations | Function identities and reviewer decisions | Allowed comparison claims | Execution truth | Reviewed declaration, not observation | Keep |
| Qualification ledger | Decide product trust/readiness | Claims, required evidence, readiness policy | Read-only verification/HIL results | Qualification decision | Workbench analysis internals | Ledger | External; never mutated by Workbench |
| Documentation | Explain current contracts and workflow | Normative architecture and operator guidance | Code/schema contracts | Human guidance | Duplicate historical narratives | This file + code/tests | Consolidate |

## Dependency and knowledge direction

```text
artifact bytes + provenance
          |
          v
canonical derived IR ----> facts / pseudo-Rust / executable reference
          |
          v
reviewed assertions (refer to evidence; do not own it)
          |
          v
raw PAC -> restricted capabilities -> HAL -> driver
                                          |
compiled vendor + production Rust --------+-> verification result
                                               |
                                      qualification ledger
```

The schema-3 composition is deliberately layered:

1. `target.toml` owns only architecture and ABI facts;
2. `ecosystem.toml` owns reusable vendor/RTOS semantic catalogs;
3. `chip.toml` owns reusable memory-map, SVD, ROM, and compiled knowledge
   selection;
4. `vendor-project.toml` references these inputs and owns only composition;
5. reviewed project workspaces own investigation-specific assertions.

Pack order is not an override mechanism. Conflicting definitions fail closed.
Two projects may reference the same chip pack without copying its address map.

## Rust ownership and capability rules

Ownership transfer, borrowing, and capability passing express hardware
authority. `split()`, `join()`, and `free()` are API choices, not mandatory
patterns. Capability boundaries follow actual exclusive/shared access needs,
not the physical SVD block layout.

- HAL must not publicly re-export a PAC owner or unrestricted register type.
- Code above HAL must not obtain an equivalent owner, including through an
  arena, facade, `Deref`, callback, or generic `with_mut` escape hatch.
- A shared mutable hardware capability names its serialization owner. A
  cloneable handle never implies unsynchronized MMIO authority.
- Multi-register sequences, polling, delays, retry limits, and recovery belong
  to HAL. Register-local fields, masks, and enum encodings belong below it.
- Protocol roles and runtime policy belong to the driver and never flow back
  into register knowledge.

Migration is vertical. The channel transaction uses a narrow borrowed
`RadioChannelHal`; this is a current slice API, not a universal split pattern.
The runtime arena stores only an opaque `RadioRuntimeOwner` and cannot yield a
PAC owner. Cold MAC, channel, DMA, IRQ, TX, AP, and STA paths now consume named
HAL operations. Powered PHY code borrows an opaque `PhyHal` with no `Deref`,
generic callback, or owner-recovery operation. PHY has no PAC dependency and
can use the capability only through named HAL operations. Repository contracts
reject the removed broad borrow APIs and any future `Deref` escape.

## Verification and qualification

Device/semantic models may describe the environment or a bounded relation;
they are not production implementations. A verification-relevant comparison ends
at compiled production Rust. Dispositions can declare what is bound and what
claim is allowed, but cannot change recorded behavior.

Workbench owns neither ledger types nor readiness policy and cannot mutate a
ledger. A future UI may display a read-only result produced by the independent
`qualification-check` tool; Workbench must not parse policy or calculate
readiness itself. An implemented function without a qualifying production
trace remains visible research coverage debt; it does not fail `project
status` or `project verify` unless a configured policy, suite, or binding
requirement makes that trace mandatory.

## Documentation policy

This file is normative for ownership and dependency boundaries.
`project-workflow.md` is normative operator workflow. `formats.md` is the
schema index. Other retained files explain one subsystem. Generated CLI help,
reports, PAC/SVD output, and manpages are generated documentation. Git history,
not checked-in migration narratives, is the historical archive.

The ESP32-S31 binding audit lives in
`verification/vendor/targets/esp32s31/audits/verification-bindings.toml`.
It records reviewed attestation/rewrite/quarantine dispositions but grants no
trust and is not a Workbench input. Qualification state remains exclusively in
the external ledger.
