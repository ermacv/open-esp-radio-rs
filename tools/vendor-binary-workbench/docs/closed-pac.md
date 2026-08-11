# Closed PAC workflow

The register workflow has two distinct Rust layers:

```text
reviewed register TOML -> clean SVD -> pac-raw (svd2rust)
                                      -> pac (closed reviewed API) -> HAL
```

`pac-raw` is generated implementation detail. It contains physical pointers,
`steal`, raw writers and other mechanisms required to implement register
access, so application and HAL crates must never depend on it. The public
`pac` crate is the policy boundary: it owns `pac-raw` privately and exports
only named capabilities, observations and reviewed transactions.

## New project order

1. Declare every code, RAM and MMIO interval in `memory.toml`. Do not infer an
   MMIO window from a discovered address after the fact.
2. Bind the private ELF/archive inputs and run `project analyze`. MMIO facts
   are evidence, not a public register catalog.
3. Run `registers review`, copy selected drafts into
   `registers/peripherals/*.toml`, then review names, offsets, access policy,
   fields and known reset information. Unknown information remains absent.
4. Add only evidence-backed transactions and public value domains to
   schema-2 `registers/api.toml`. A writable
   bit set, enum value, complete fixed image or opaque value domain is an API
   decision and must cite its source evidence.
5. Run `project publish`. `[registers.svd]` produces portable hardware
   metadata, `[registers.pac-raw]` produces the internal svd2rust source, and
   `[registers.api].output` produces a checked-in closed-facade module.
6. Expose reviewed operations from the closed `pac` crate and implement the
   HAL only in terms of those operations. Use compiled comparison to qualify
   the resulting vendor-to-Rust transaction boundary.

Discovery therefore comes before SVD, but SVD generation is not the end of
review. Observed masks can suggest field boundaries; they do not prove field
names, W1C behavior, reset values or that every legal value has been seen.

## Public API rules

- No physical address, raw pointer, raw register block, `steal`, or generic
  `write_bits(u32)` crosses the closed PAC boundary.
- Read results may expose a numeric snapshot for diagnostics. The API does not
  provide the inverse constructor for a writable value.
- Bit masks use an opaque flags type whose public constructors are reviewed
  constants and composition operations. A caller cannot invent another bit.
- Bounded fields use checked newtypes or enums. Values which are truly opaque
  (for example calibration table words) use a register-specific newtype, not
  a naked `u32` shared by unrelated registers.
- W1C/status acknowledgement consumes the snapshot returned by its paired
  read where the hardware protocol permits this.
- Multi-register sequencing and ownership transitions remain explicit
  capability methods. The HAL may name policy, but it cannot reconstruct
  register addresses or raw images.

Combining flags in Rust is an atomic *value construction*, not necessarily an
atomic MMIO read-modify-write. The tool must not emit RISC-V AMO instructions
for device memory unless the target memory map and hardware documentation
explicitly qualify them. Concurrency is instead controlled by unique
capabilities, interrupt ownership, critical sections, or a reviewed hardware
SET/CLEAR alias.

Schema 2 currently generates four closed value-domain forms from
`registers/api.toml`: composable flags, finite enums, inclusive checked ranges
and register-specific opaque newtypes. A target must still bind each generated
domain to a reviewed capability method; merely discovering a field never
creates a writable public operation.

For domain-bound operations the generated module also owns the only typed
bridge to `pac-raw`. Handwritten ownership and sequencing methods remain in the
closed PAC, but cannot accidentally pass an arbitrary integer to that reviewed
register write.

## ESP32-S31 reference boundary

The repository keeps generated source in
`driver/chips/esp32s31/pac-raw` and the application-facing API in
`driver/chips/esp32s31/pac`. `open-esp-radio-esp32s31-pac-raw` is private and
non-publishable. An architecture test rejects any other driver crate which
depends on it. The MAC interrupt-enable image is represented by
`MacInterruptMask`; its integer constructor is private, while status is read
through `MacInterruptEvents` and finite snapshot types.

The current reviewed transaction pack still contains older dynamic `u32`
operations used inside the closed crate. Migrating those inputs to generated
flags, enums, bounded values and register-specific opaque domains is tracked
as remaining PAC work; they must not be re-exported as raw public writers.
