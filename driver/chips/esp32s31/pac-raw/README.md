# ESP32-S31 raw PAC

This crate is generated implementation detail for
`open-esp-radio-esp32s31-pac`. It intentionally exposes the low-level
svd2rust API required to implement reviewed transactions.

Do not depend on it from HAL, driver, application, example or HIL crates.
Physical pointers, `steal` and raw register writers are not product APIs. The
workspace architecture test permits this dependency only from the adjacent
closed `pac` crate.

Regenerate it through the ESP32-S31 Workbench project:

```console
cargo vendor-binary-workbench project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```
