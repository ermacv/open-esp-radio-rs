# ESP32-S31 migration analysis tools

Historical host-side analyzers and strict-policy audits transferred with the
hybrid runtime source.

They are not Cargo workspace members and currently retain assumptions about
the old `esp-wifi-sys` artifact layout. Keep them as provenance and porting
inventory until equivalent source-only map/register audits live under
`tools/`. They must never cause the open driver to link a vendor archive.
