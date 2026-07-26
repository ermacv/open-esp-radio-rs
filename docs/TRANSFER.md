# Repository transfer

Transferred from the experimental `esp-wifi-sys` worktree:

- the complete source-only ESP32-S31 PHY frontier and its 233 host tests;
- the async cold-PHY executor and identity-bound hardware actions;
- all project audit and ownership-migration documents;
- the complete former Rust async runtime/MAC/WPA workset under `migration/`.
- the ESP32-S31 map/state/strict analysis utilities used during that work.

The migration directory is deliberately excluded from the workspace because
it still records historical blob interposition. It is a porting inventory,
not a linkable driver.

Not transferred:

- vendor libraries, ROM ELF files, extracted objects or binary data;
- generated vendor headers;
- router credentials, HIL secrets or machine-specific absolute paths;
- the `esp-wifi-sys` crates themselves.

The old repository has no dependency on this workspace. Future dual-driver
selection and hardware flashing are owned by the separate `esp32s31_rust`
application project.
