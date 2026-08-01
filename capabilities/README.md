# Capability progress ledger

The checked-in ledger is the primary completion metric for the supported
driver path. Vendor-function totals remain useful validator throughput data,
but they do not prove that a usable radio capability is complete.

Run the fail-closed check from the repository root:

```console
cargo capability-ledger check \
  --manifest capabilities/esp32s31-wifi-sta.ledger
```

Each capability has five independent axes:

- `implementation`: a production owner exists under `crates/*/src`;
- `host-proof`: named host tests exercise the capability contract;
- `vendor-proof`: roots have executable validator contracts, are only mapped
  to reviewed source anchors, or are explicitly outside vendor comparison;
- `hil-proof`: a dated hardware record contains the named qualification ID;
- `async-proof`: waits are bounded scheduling edges or not applicable.

Only terminal values make a capability ready. Every non-terminal axis must
carry a stable gap ID. Dependencies are checked for missing nodes and cycles;
`proof-ready` means the capability's own five axes are terminal, while
`ready` additionally requires every dependency to be ready. The manifest also
declares every required root separately so deleting a difficult capability
cannot improve the summary.

A historical HIL record supports `partial`, not `qualified`, after any owner
or integration boundary named by that record changes. Promotion back to
`qualified` requires repeating the cell against the current tree and adding a
new immutable record (or an explicit current-revision addendum).

Owner, test, source-anchor and HIL references are checked against their real
repository files. A `vendor-root` must be an explicit entry in the PHY
disposition manifest; `vendor-proof qualified` additionally requires its Rust
component and executable semantic/effect contract.
