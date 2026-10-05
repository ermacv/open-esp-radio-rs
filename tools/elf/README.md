# oer-elf

The one view of ELF files and static archives that every host tool reads
through; nothing else parses `llvm-nm` or `llvm-objdump` output or walks
`object` for symbols, sections or relocations.

- `Elf::parse` (any ELF), `Elf::executable` (a static little-endian RV32
  image), `objects(bytes)` (each ELF member of an archive, or the one file)
  and `is_binary`.
- Symbols: `symbols()`, `symbol(name)`, `address(name)`, `addresses()`,
  `demangle(name)`. `functions()` merges every code symbol at one address
  into one `Function` with all its names (identical-code-folded aliases),
  and extends an unsized entry to the next code symbol.
- Sections: `sections()`, `section(index)`, `section_by_name`, with their
  allocation, write, execute, TLS and `NOBITS` flags and bytes.
- Relocations: `relocations(section)` and `target_address`. `rv32` is the
  one RV32 relocation table: each type's patched `Field` and its target's
  `Role` (call, jump, branch, absolute or PC-relative address halves, data
  word, hint, other), and `mask`, which clears exactly the patched bits.
  An unknown type masks its whole word. The vendor fingerprints, the stack
  analysis, the lifter and the vendor report all classify relocations
  through it; a test pins every row.
- `code(member)`: the defined functions of a relocatable object with their
  bytes and relocation `Site`s, each target resolved to an offset in the
  function's own section (`Reference::Local`), a named symbol, an anonymous
  section, or nothing. The vendor fingerprints are computed over it
  ([`oer-vendor-provenance`](../vendor-provenance/README.md)).
- `dwarf::Symbolizer::frames(address)`: the inline chain at an address with
  each frame's demangled function, file and line. `dwarf::load` gives the
  debug information itself to analyses that read DWARF entries.

It has no repository path dependency, so Blobray's standalone extraction
takes it by path beside the RV32 crates. Blobray's own artifact inventory
(`tools/blobray/crates/artifacts`) stays separate: it streams member and
section bytes under Blobray's working-memory and cancellation budget and
keeps artifact, member and symbol-table identities for evidence, which a
whole-file view does not.
