# Vendor analysis model

Architecture-neutral symbolic values, observable-effect/reference IR and SVD
derived MMIO catalogs shared by instruction backends and knowledge providers.

The crate does not decode instructions, name physical argument registers, bind
vendor artifacts, or select a chip-specific provider.

Reviewed memory-access classifications and compressed-pointer layout
descriptors also belong here. Knowledge providers can declare these facts
without depending on an instruction backend or executable model provider.
The RISC-V backend recognizes exact bit provenance against a pointer
descriptor; the neutral descriptor itself does not execute that analysis.
