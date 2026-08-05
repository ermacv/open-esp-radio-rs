# ESP32-S31 vendor PHY oracle transfer

The vendor PHY differential oracle now lives entirely in this repository at
`hil/vendor-oracle/esp32s31`. It is a separate Cargo workspace and is excluded
from both the root source-only workspace and `hil/esp32s31`. Normal metadata,
tests, HIL builds and published crates therefore do not resolve `esp-phy`,
`esp-rtos` or `esp-wifi-sys`.

The transferred runtime is byte-for-byte source-equivalent to
`esp32s31_rust` commit `b637aee74dd1e31e61fcef2a90b2827cfa1e7eea` apart from:

- importing the already transferred console and ESP-HAL radio adapter locally;
- replacing the board-crate page-size constant with its exact 64-KiB value;
- adding the inert deterministic flash-reference section required by the
  current shared S31 bootstrap linker.

No vendor PHY call, wrapper, transition, address probe or open-MAC handoff was
changed during the transfer.

## Reproducible entry points

```text
cargo hil oracle verify
cargo hil oracle build
cargo hil oracle flash --port /dev/ttyACM0
```

`verify` computes SHA-256 in the Rust runner and checks all 20 ignored local
ROM/archive evidence files against
`hil/vendor-oracle/esp32s31/oracles.lock`. `build` uses the separately locked
oracle workspace and locates the matching hard-float Espressif `libgcc.a`
without a shell script. `flash` writes the shared HIL partition table, the
oracle image to `ota_0`, and a valid IDF OTA selector.

## Build and boot evidence

- Oracle image size: 203,648 bytes.
- Oracle application SHA-256:
  `950f41accbe6583f71a2355d9e6065836e5ba29e4f1974a07ebdcffd7ef7e4fa`.
- Oracle ELF SHA-256:
  `a3060a6eb1f8043461ad43e30814e23b9c048a42a32fa477f4ba039f00154c8a`.
- The image booted from `ota_0`, completed vendor cold PHY initialization and
  reached `OPEN_RADIO_ORACLE_HIL stage=vendor-phy-ready`.
- All wrapped RF/TXDC boundaries executed, including both the cold-init TXDC
  call and the explicit repeat call.
- The vendor-PHY/open-MAC diagnostic then published the authentication frame,
  but hardware completion status was 5 and no RX frame arrived; it ended at
  `vendor-phy-open-mac-auth-timeout` with `frames=0`.

The last item is preserved as an unresolved oracle result, not reported as a
successful association. It is the existing first-packet/vendor-PHY-to-open-MAC
boundary and must be investigated separately; the source-only HE20 HIL remains
the authoritative working scan/connect/data path.
