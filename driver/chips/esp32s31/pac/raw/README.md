# ESP32-S31 raw PAC

This crate is restricted implementation detail for
`open-esp-radio-esp32s31-pac`. It intentionally exposes the low-level
svd2rust API required to implement reviewed transactions.

`src/lib.rs` is the generated backend. The three adjacent IEEE 802.15.4
sidecars are handwritten: `ieee802154_mac_ownership.rs` implements the affine
task/interrupt split and reunion; the two `*_validation.rs` modules contain
feature-gated validation transactions. They each deny `unsafe_code` and
`unsafe_op_in_unsafe_fn`, allowing unsafe only on documented reviewed
operations. Their tests live in child files. Do not treat these sidecars as
generated code or exempt them from ownership review.

Do not depend on it from HAL, driver, application, example or HIL crates.
Physical pointers, `steal` and raw register writers are not product APIs. The
workspace architecture test permits this dependency only from the adjacent
closed `pac` crate.

Regenerate it from the reviewed source-only composition:

```console
cargo blobray registers generate-pac-raw \
  --project registers/esp32s31/publication/vendor-project.toml
```

Use `--check` to validate reproducibility without overwriting the output.
The [semantic PAC map](../README.md) identifies the register model, API policy
and the other published artifacts.
