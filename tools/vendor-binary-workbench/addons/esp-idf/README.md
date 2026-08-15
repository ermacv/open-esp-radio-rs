# ESP-IDF semantic add-on

This add-on owns reusable ESP-IDF ABI identities and meanings that are shared
between chips. Known RTOS, logging, NVS, timing, and OSI calls are semantic
boundaries: analysis records the call, signature, arguments, return contract,
and modeled effects, but does not treat their implementation body as part of
the caller being reconstructed.

Chip addresses and ROM identities do not belong here. Function-table layouts
remain reviewed interface bindings; they may reference the reusable operation
vocabulary without turning that vocabulary into evidence for a concrete slot.
