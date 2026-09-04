# Function review and generated route witnesses

Function packs bind human meaning to authenticated inputs and generated linked
IR. Names, ABI roles, context layouts, selector meanings, callback identities,
and rationale belong in the reviewed pack. The pack does not need to copy each
address or the pretty-printed value that the analyzer can recover.

Schema 11 accepts optional call-site selectors for static callback and broker
subscription routes:

- Omit `binding-site`, `receive-site`, `run-site`, `dispatch-site`, `domain-site`,
  or `case-handler-site` to select the unique call to the specified function or
  semantic operation. Zero or multiple matches are errors. An indirect call or
  one without an observed address cannot satisfy the selector.
- Omit static-route `dispatch-sites` to use all current matching calls. The
  analyzer checks every selected call, including shared-object and argument
  exactness requirements. Selection is ordered by address and limited to 64
  calls. Explicit sites still select only those occurrences; empty or duplicate
  lists are errors.
- Omit `upstream-sites` to require one unique direct internal call for each
  edge of `upstream-chain`. The chain currently remains an explicit reviewed
  path selection; automatic discovery of the entire chain is separate work.
- Omit `binding-callback-store-site` to infer the unique observed store of the
  reviewed callback into the subscribed object at the reviewed field offset.
  Store width, pointer value, object identity, and CFG ordering checks remain.
- Omit broker `payload-value` to derive the current exact argument value. The
  reviewed `payload-role` describes its meaning. The report retains the
  generated value; an unknown or varying argument remains an error.

Explicit addresses and legacy `payload-value` assertions remain supported and
must match. They are useful when a reviewer must distinguish otherwise
ambiguous occurrences. Incorrect types are errors, never treated as omission.
Input digest checks remain mandatory; inference does not carry review to an
unreviewed binary.

For example, a binding selector can describe the call and its argument roles
without copying its current instruction address:

```toml
binding-profile = "controller"
binding-source = "vendor"
binding-entry = "vendor::initialize"
binding-operation = "event.init"
binding-object-argument = 0
binding-callback-argument = 1
```

Workspace validation and flow investigation share one bounded call selector.
The broker terminal reachability check uses breadth-first traversal of observed
internal direct calls. It distinguishes an absent path from exhausted depth,
node, or edge budgets. This establishes structural reachability only; callback
delivery, subscriber lifetime, and path feasibility retain their independent
proof obligations.
