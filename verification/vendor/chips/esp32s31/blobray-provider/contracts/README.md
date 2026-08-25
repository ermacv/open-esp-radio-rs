# Reusable ESP32-S31 rev0 contracts

This crate is the compiled companion of the `esp32s31-rev0-radio-v1` chip
pack. It contains only contracts that the reviewed chip pack can reuse across
investigations of that ROM revision:

- the cold thirteen-slot ROM PHY function table;
- the cold entry contract and neutral `none` entry state;
- the ROM `ets_printf` diagnostic boundary.

Linked archive callbacks, registered mutable-table state, SDK callback-table
layouts and private vendor-library diagnostics are not chip facts. They remain
in the investigation provider and may compose with this crate only through an
explicit compiled-provider `extends` relationship.
