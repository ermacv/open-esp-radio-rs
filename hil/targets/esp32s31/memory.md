# HIL stack observations

## IRQ stack evidence

The shared platform paints dedicated SRAM IRQ stacks before interrupt admission.
The Bluetooth images sample core 0's IRQ stack on its hart in thread mode
(`agent/src/stack_evidence.rs`) and return it with their stack queries.

The [stack policy](../../../platform/esp32s31/stack.toml) sets the interrupt stacks' `minimum_free_bytes`; its current
reserve is an engineering guard, not a qualified nesting bound. A sampling
request that cannot complete fails explicitly. Read the
[wire contract](../../protocol/diagnostics.md#stack-measurements) for absence semantics
and the instrumentation's effect on interrupt latency.
