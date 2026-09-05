# Reviewed register descriptions and publication

This tree owns the hardware descriptions and reviewed policies shared by
production PAC publication and vendor analysis. It contains source data,
provenance and checked generated catalogs; it is not a runtime crate or a
vendor investigation.

The [ESP32-S31 model](esp32s31/README.md) distinguishes editable hardware
semantics, publication policy, upstream inputs and generated outputs. Runtime
register owners and both generated Rust outputs remain in
[`driver/chips/esp32s31/pac`](../driver/chips/esp32s31/pac/README.md).

Generic loading, validation and generation remain in
[`tools/blobray`](../tools/blobray/README.md). A vendor project references the
same reviewed model while selecting its own artifact context and comparison
policy. Neither source generation nor binary analysis decides product readiness;
that authority remains with [qualification](../qualification/README.md).
