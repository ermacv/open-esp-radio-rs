# HIL agent

`oer-hil-agent` holds the HIL target logic that does not
depend on the chip: console serialization, the reset-retained post-mortem
record, task liveness slots, the request flags of a cross-hart sample line
(no image runs the hang watchdog or the profiler that used them), the
platform's own trace events and the secure-GATT evidence. The [esp32s31 HIL agent](../targets/esp32s31/README.md) composes these
modules with its radio, executor and console. Host tests exercise the same
compiled code the firmware links.

| Feature | Modules |
| --- | --- |
| none | `console`, `liveness`, `postmortem`, `profile`, `sample_request`, `trace` |
| `secure-gatt` | `bluetooth_gatt::secure`, with the protocol's `bluetooth` family |

The feature enables the matching radio family of the HIL protocol, so an
image that does not select it compiles none of that family's protocol
modules.

Run the tests for each configuration:

```console
cargo test -p oer-hil-agent
cargo test -p oer-hil-agent --features secure-gatt
```
