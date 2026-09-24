# Blobray verification

`blobray-verification` compares concrete observations through domain values. It
has no filesystem, store, executor, provider-selection or publication authority.
Application supplies the two observations and the shared operation control.

The `ordered-mmio-fence-delay-u32/model-4` relation compares the exact ordered MMIO reads,
writes, fence and modeled delay events, and optionally the low 32-bit return value. Ordinary
RAM, call boundaries, elapsed time and the high return register are outside this
relation. No selected observable event is normalized away. A completed shorter trace contradicts
an observed extra event on the other side even if that other execution stopped
incomplete. An incomplete shorter prefix cannot prove a length difference.
Unknown required returns and unfinished executions cannot yield `MATCH`.
Already observed differing return values remain `DIFF` even if a model has
unconsumed obligations; the adjacent completeness remains false.

`DIFF` retains the first differing event index or a return mismatch. `INCOMPLETE`
retains the execution gaps in the adjacent observations. The caller preserves
those observations; the verifier neither mutates nor substitutes them. Every
compared event consumes the same run's work budget. Exhaustion is an error, never
a verdict. Application aggregates cases with `DIFF` taking precedence over
`INCOMPLETE`, and publishes through the ordinary durable job lifecycle.

The result applies to the explicitly enumerated cases and caller-declared
compiled binding. It is neither whole-domain equivalence nor qualification.
See [concrete execution](../../next/README.md#concrete-execution-and-comparison)
for the recipe, session lifetime and supported environment.

The relation accepts explicitly completed symbol/call
goals when return comparison is disabled and both outcome kinds agree. An early
return before a goal remains incomplete. Event-prefix differences remain evidence;
equal incomplete prefixes cannot match. No target body or suspended continuation is
inferred from reaching an early boundary.


The model-4 relation requires a completed code goal and satisfied model obligations due at that phase. An open session transcript can continue warm; unconsumed values at closure cannot MATCH even when both programs return. A known observed event difference remains DIFF independently of incomplete coverage.


The model-4 observable stream also includes explicit modeled microsecond delays. Call argument/output/allocation/return records remain retained evidence outside this relation; no address normalization or pointer-layout equivalence is inferred. Difference indices count the selected observable stream.
