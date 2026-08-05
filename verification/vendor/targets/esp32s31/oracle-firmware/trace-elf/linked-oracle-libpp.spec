# Optional libpp linked view used by the interrupt/DMA migration pilots. The
# raw archive remains the authoritative function inventory.
schema 1
source libpp
linker-script ../../../../../../hil/targets/esp32s31/linker/rom/esp32s31-eco0.x
linker-script link.x
archive ../../../../../../_oracles/libpp.a
stub-symbol __adddf3
stub-symbol __clzsi2
stub-symbol __ctzsi2
stub-symbol __extendsfdf2
stub-symbol __ffssi2
stub-symbol __fixdfsi
stub-symbol __floatsidf
stub-symbol __floatundisf
stub-symbol __floatunsidf
stub-symbol __muldf3
stub-symbol __popcountsi2
stub-symbol __subdf3
stub-symbol __truncdfsf2
stub-symbol __udivdi3
stub-symbol __umoddi3
# Explicit linked-view fixtures for optional vendor diagnostics/statistics.
# They are not production driver state. The generation contract must classify
# every reachable access before any such behavior may be omitted or replaced.
fixture-data-symbol esp_test_rx_statistics 8 4
fixture-data-symbol g_dbg_interp_tsf 4 4
fixture-data-symbol g_dbg_interp_tsf_end 4 4
whole-archive true
emit-relocations true
gc-sections false
unresolved-symbols ignore-all
