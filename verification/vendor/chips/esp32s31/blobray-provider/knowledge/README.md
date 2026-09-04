# ESP32-S31 rev0 declarative knowledge

This crate publishes the reviewed 40 MHz crystal semantic declaration and
compressed-pointer encoding facts, with entry/diagnostic contracts supplied
by the sibling contracts crate. It has no executable addon dependencies and
installs no summary hooks.

The sibling [`models`](../models/README.md) crate interprets these facts and
composes the reusable C/ESP-IDF runtime adapters. The host chooses that
executable provider explicitly. Exact private function reconstructions remain
in the investigation model crate.
