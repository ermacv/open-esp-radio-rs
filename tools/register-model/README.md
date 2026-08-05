# Register model library

`open-esp-radio-register-model` owns only the portable schema-2 hardware
description:

- safe loading of a multi-file TOML model;
- CMSIS-SVD data structures, arrays, clusters, fields and enumerations;
- structured review records kept outside exported hardware descriptions;
- deterministic clean SVD materialization and expanded register identities.

It does not know about ELF files, discovery facts, ESP32-S31, PAC helper
semantics or output paths. The vendor validator composes it with observed MMIO
facts; `pac-gen` composes it with the ESP32-S31 target add-on.

The format and editing workflow are documented in
[`../vendor-code-validator/docs/register-workspace.md`](../vendor-code-validator/docs/register-workspace.md).
