# Repository transfer

Transferred from the experimental `esp-wifi-sys` worktree:

- the complete source-only ESP32-S31 PHY frontier and its 237 host tests;
- the async cold-PHY executor and identity-bound hardware actions;
- all project audit and ownership-migration documents;
- the remaining unported Rust async runtime/MAC/WPA workset under
  `migration/esp32s31-hybrid-runtime`.

The migration directory has no Cargo manifest because it still records
historical blob interposition and is intentionally incomplete after qualified
modules move out. It is a porting inventory, not a linkable driver. Qualified
PHY and passive-scan duplicates have been deleted; Git history preserves the
pre-cleanup workset.

The old blob map/state/strict analyzer sources were removed after the library
analysis phase ended. Their generated reports remain under `docs/`, and Git
history preserves the generators.

Not transferred:

- vendor libraries, ROM ELF files, extracted objects or binary data;
- generated vendor headers;
- router credentials, HIL secrets or machine-specific absolute paths;
- the `esp-wifi-sys` crates themselves.

The old repository has no dependency on this workspace. Future dual-driver
selection and hardware flashing are owned by the separate `esp32s31_rust`
application project.
