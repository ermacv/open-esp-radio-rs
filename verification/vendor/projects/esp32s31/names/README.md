# ESP32-S31 vendor function names

These maps give the source names of generated function names in the pinned
ESP32-S31 Controller archive
[`espressif/esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`](https://github.com/espressif/esp32s31-bt-lib/tree/7f20740dd66ee774ffce5db0b55507892551aa31).
[`oer-symbol-lineage`](../../../../../tools/symbol-lineage/README.md) carries
the names from the initial archive commit `31c3094`, which has source names,
through every later commit that changes each library.

| Map | Library | Recovered | Without a name |
| --- | --- | --- | --- |
| [`libble_app.toml`](libble_app.toml) | `libble_app.a` | 2420 | 120 |
| [`libbtdm_common.toml`](libbtdm_common.toml) | `libbtdm_common.a` | 250 | 15 |

`libbredr_app.a` has source names in every commit and needs no map. Each map
records the SHA-256 of every archive revision; the last one is the pinned
archive. A commit that leaves a library unchanged does not appear, so the
last `libble_app.a` revision is the earlier commit with identical bytes.

Names were produced with `--obfuscated-prefix r_sym_` and the default policy.
Each entry's `weakest` field is the weakest evidence of its chain:

| Evidence | `libble_app.toml` | `libbtdm_common.toml` |
| --- | --- | --- |
| `same-name` or `exact-body` only | 2097 | 243 |
| `call-graph` | 268 | 5 |
| `similar` | 55 | 2 |

`similarity-ppm` is the lowest body similarity along the chain. A name does
not describe current behavior: statements about a function still come from
its current body. Regenerate the maps with the command in the tool README
after a new archive revision, and record names of functions without an entry
by manual review in the reference that uses them.
