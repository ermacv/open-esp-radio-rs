profile rom-sta-tsf-snapshot
vendor-source rom
vendor-symbol hal_get_sta_tsf
rust-symbol open_rom_power_tsf_trace_hal_get_sta_tsf
compare-return false

case neither-output
arg 0
arg 0
mmio 0x2010d814=0xa5a50010

case low-output
arg 0x3fff0000
arg 0
mmio 0x2010d814=0xa5a50010
mmio 0x2010d820=0x01234567
ram 0x3fff0000=0
observe 0x3fff0000=4

case high-output
arg 0
arg 0x3fff0004
mmio 0x2010d814=0x5a5a0011
mmio 0x2010d824=0x89abcdef
ram 0x3fff0004=0
observe 0x3fff0004=4

case both-outputs
arg 0x3fff0000
arg 0x3fff0004
mmio 0x2010d814=0x12340010
mmio 0x2010d820=0x76543210
mmio 0x2010d824=0xfedcba98
ram 0x3fff0000=0
ram 0x3fff0004=0
observe 0x3fff0000=8
