# Verification

This tree contains checked, shareable inputs for comparing source-owned Rust
behaviour with caller-supplied vendor code. It does not contain production
driver code, board qualification scenarios or private vendor artifacts.

```text
verification/
└── vendor/
    └── targets/
        └── esp32s31/
            ├── target, memory, profile, disposition and baseline pack
            ├── probes/             source-owned compiled comparison probes
            └── oracle-firmware/    isolated opt-in vendor-linked firmware
```

The generic engine is temporarily implemented by
[`tools/blobray`](../tools/blobray/README.md).
Target packs live here so the engine does not own chip identity, ABI layout,
SVD selection or reviewed vendor-function policy.

Use the ESP32-S31 project entry point for repository workflows:

```console
cargo blobray project doctor \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Artifact paths, revisions and authentication remain caller-owned. The target
pack may report evidence identities but must not select a private input by a
hard-coded path or digest.

Evidence strength and the only path from a Blobray comparison to driver
readiness are defined in the canonical
[verification and qualification contract](../docs/VERIFICATION_AND_QUALIFICATION.md).
