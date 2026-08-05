# This checked-in fixture selects the current PHY source. Set
# OPEN_RADIO_LINKED_ORACLE_SPEC to a caller-owned spec to link libpp,
# libnet80211, libwpa or another authenticated source with the same builder.
schema 1
source libphy
linker-script ../../../../../../hil/targets/esp32s31/linker/rom/esp32s31-eco0.x
linker-script link.x
archive ../../../../../../_oracles/libphy.a
whole-archive true
emit-relocations true
gc-sections false
unresolved-symbols ignore-all
