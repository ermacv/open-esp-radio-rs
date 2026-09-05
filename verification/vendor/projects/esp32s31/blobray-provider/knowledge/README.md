# ESP32-S31 investigation declarative knowledge

This crate owns reviewed semantic declarations, artifact-local RAM access
classifications and the investigation's ABI/entry contracts. The `pp_post`
declaration records event meaning; it contains neither an instruction trace
nor an executable body matcher.

It has no dependency on model providers, executable C/ESP-IDF addons or the
execution-model interpreter. Its backend dependency supplies typed memory
classification records. Selecting these declarations alone installs no summary
hooks and cannot replace function control flow.

The sibling [`models`](../models/README.md) crate owns all temporary executable
reconstructions and their applicability checks. The host composes facts and
models explicitly. See [`../OWNERSHIP.md`](../OWNERSHIP.md).
