# crates/

Production libraries, one directory per layer; the
[source map](README.md#source-map) names every package's responsibility.

| Directory | Layer |
| --- | --- |
| `protocols/` | Sans-IO protocol state machines and the ports their drivers implement |
| `services/` | Executor-free drivers that wait on ports and feed the state machines |
| `hardware/` | Chip and family PAC, HAL, PHY and radio drivers |
| `runtime/` | Executor-independent async radio execution over the `oer-time` ports |
| `adapters/` | Executor, `embassy-time` and board bindings |
| `oer/` | The public facade; no internal package depends on it |

## Rules

- Dependencies follow the [layer table](../docs/architecture.md#layer-dependencies),
  independently of directories.
- A `protocol` package never awaits: time and events enter as values
  ([sans-IO](../docs/architecture.md#sans-io-protocols-executors-and-time)).
  Only adapters and the facade use `embassy-time` or an executor.
- Standard `rustfmt` and Rust naming. Typed ownership and state transitions,
  not raw addresses or register images.
  Find a PAC accessor by grepping `hardware/<chip>/pac/src/generated.rs` or
  `hardware/<chip>/pac/raw/src/lib.rs` by explicit path; never read them whole.
- `unsafe` follows [UNSAFE.md](UNSAFE.md): `// SAFETY:` on every block.

## Tests

Every behavioral change gets a focused test in the module (`#[cfg(test)]`) or
the crate's `tests/`. Never test generated register addresses, masks, shifts,
field positions or PAC type names. Memory-protocol tests check behavior and
ownership, not the implementation's raw image or layout constants. Runtime
tests use the virtual clocks of `oer-time-virtual`.

## Commands

- `cargo test -p <package> <test>`.
- `cargo hil images check --class <class> --type-check` to type-check an image class.
- `cargo xtask doc` after an API change.
