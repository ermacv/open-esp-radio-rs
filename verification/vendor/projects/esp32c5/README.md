# ESP32-C5 portability fixture

This target proves that Blobray composition is not tied to ESP32-S31. It
shares the RV32 backend and ESP-IDF family knowledge pack, but deliberately
contains no chip addresses, register names, SVD, compiled provider, or PAC
publication policy.

Bind a private image through an untracked run spec. Add a reviewed memory map,
chip knowledge, register model, and PAC allowlist only as independent evidence
becomes available; unknown facts must remain unknown.
