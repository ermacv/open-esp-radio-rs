# Both source-owned bool inputs and distinct initial images prove the
# non-symmetric vendor transaction: disable clears bit 29 at 0x2010d858, but
# both branches set bit 21 at 0x2010d830.
profile libpp-sta-tsf-wakeup
vendor-source libpp
vendor-symbol hal_set_sta_tsf_wakeup
rust-symbol open_libpp_power_tsf_trace_hal_set_sta_tsf_wakeup
compare-return false
arg-range 0 0 1

case disabled
arg 0
mmio 0x2010d858=0xffffffff
mmio 0x2010d830=0x00000000

case enabled
arg 1
mmio 0x2010d858=0x01234567
mmio 0x2010d830=0x5a1a55aa
