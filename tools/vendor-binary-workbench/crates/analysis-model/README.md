# Vendor analysis model

Architecture-neutral symbolic values, observable-effect/reference IR and SVD
derived MMIO catalogs shared by instruction backends and platform harnesses.

The crate does not decode instructions, name physical argument registers, bind
vendor artifacts, or select a chip-specific harness.
