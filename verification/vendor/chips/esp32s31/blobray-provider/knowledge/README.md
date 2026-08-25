# Reusable ESP32-S31 rev0 knowledge add-on

This provider composes generic C and ESP-IDF semantics with only one reviewed
chip-wide direct semantic: ESP32-S31's fixed 40 MHz crystal query. Its entry
and diagnostic contracts come from the sibling rev0 ROM contract crate.

Exact function bodies, linked addresses, relocation layouts and private
archive ABIs are deliberately absent. Those facts cannot be selected by the
chip pack unless separate evidence proves their applicability across every
investigation using the pack.
