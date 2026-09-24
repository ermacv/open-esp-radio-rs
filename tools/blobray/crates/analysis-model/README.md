# Vendor analysis model

This package belongs to the retained vendor-investigation engine. The current
`cargo blobray` command uses [Next](../../next/README.md); it does not load this
package implicitly. This page describes the retained library contract.


Architecture-neutral symbolic values, observable-effect/reference IR and SVD
derived MMIO catalogs shared by instruction backends and knowledge providers.

The crate does not decode instructions, name physical argument registers, bind
vendor artifacts, or select a chip-specific provider.

Symbolic addresses and relocated memory roots carry a mandatory
`SymbolReference`. Arithmetic, pointer-load provenance and canonical value keys
retain it, so equal names do not merge different captured symbol occurrences.
An unknown reference remains explicit; the value layer does not infer linker
selection or promote a global definition to a resolved target.

Reviewed memory-access classifications and compressed-pointer layout
descriptors also belong here. Knowledge providers can declare these facts
without depending on an instruction backend or executable model provider.
The RISC-V backend recognizes exact bit provenance against a pointer
descriptor; the neutral descriptor itself does not execute that analysis.
