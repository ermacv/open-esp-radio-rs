# PHY archive audit

`oer-check-phy-archive` belongs to the verification application. It audits
the compiled symbols of a source-only PHY archive, rejecting radio ROM or
vendor ABI symbols whether defined or referenced. Unresolved references
must belong to the allowed source crates or the reviewed core and compiler
support functions.

The [xtask PHY check](../xtask/README.md#firmware-builds-and-image-checks)
passes the archive and its allowed source crates to this tool as a separate
process. This keeps the ELF reader outside the gate's dependency graph.

```console
cargo run -p oer-check-phy-archive -- <archive.rlib> --source <crate>
```

Repeat `--source` for each allowed crate. Crate names accept hyphens and
are normalized to their Rust namespace spelling.

ELF and archive containers are read through [`oer-elf`](../elf/README.md).
LLVM bitcode members use `llvm-nm` from the active Rust toolchain. Unknown
members, compiled ELF members without symbol tables, tool failures and
malformed symbol output fail the audit.

The tests compile native ELF and LLVM bitcode fixtures and check the symbol
policy and invalid archive handling:

```console
cargo test -p oer-check-phy-archive
```
