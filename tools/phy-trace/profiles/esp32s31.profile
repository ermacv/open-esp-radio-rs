# Concrete compiled-equivalence scenarios. This is validator input, not a
# prose function ledger. Every conditional branch outcome must be covered.

profile rom-bluetooth-index-to-baseband
vendor-source rom
vendor-symbol phy_bt_index_to_bb
rust-symbol open_phy_trace_ret_bt_index_to_bb
compare-return true

case zero
arg 0

case index-one
arg 1

case index-two
arg 2

case outside-domain
arg 0xffffffff

profile rom-bluetooth-baseband-to-index
vendor-source rom
vendor-symbol phy_bt_bb_to_index
rust-symbol open_phy_trace_ret_bt_bb_to_index
compare-return true

case zero
arg 0

case baseband-one
arg 0x80

case baseband-two
arg 0x100

case outside-domain
arg 0xffffffff

profile frequency-band-registers
vendor-source rom
vendor-symbol phy_freq_band_reg_set
rust-symbol open_phy_trace_freq_band_reg_set
compare-return false

case disabled
arg 0
mmio 0x20107030=0xffffffff
mmio 0x20107ce4=0xffffffff

case enabled
arg 1
mmio 0x20107030=0xffffffff
mmio 0x20107ce4=0

profile archive-watchdog-reset-enable
vendor-source archive
vendor-symbol bb_wdt_rst_enable
rust-symbol open_phy_trace_bb_wdt_rst_enable
compare-return false

case disabled
arg 0
mmio 0x20107c40=0xffffffff

case enabled
arg 1
mmio 0x20107c40=0

profile archive-watchdog-interrupt-enable
vendor-source archive
vendor-symbol bb_wdt_int_enable
rust-symbol open_phy_trace_bb_wdt_int_enable
compare-return false

case disabled
arg 0
mmio 0x20107c40=0xffffffff

case enabled
arg 1
mmio 0x20107c40=0

profile archive-watchdog-timeout-clear
vendor-source archive
vendor-symbol bb_wdt_timeout_clear
rust-symbol open_phy_trace_bb_wdt_timeout_clear
compare-return false

case initially-clear
mmio 0x20107c40=0

case initially-set
mmio 0x20107c40=0xffffffff

profile archive-watchdog-status
vendor-source archive
vendor-symbol bb_wdt_get_status
rust-symbol open_phy_trace_ret_bb_wdt_get_status
compare-return true

case clear
mmio 0x20107c08=0

case set
mmio 0x20107c08=0xffffffff

profile rom-rx-11b-optimization
vendor-source rom
vendor-symbol phy_rx_11b_opt
rust-symbol open_phy_trace_rx_11b_opt
compare-return false

case disabled
arg 0
mmio 0x20107044=0xffffffff
mmio 0x20107124=0xffffffff
mmio 0x20108004=0xffffffff
mmio 0x20107104=0xffffffff

case enabled
arg 1
mmio 0x20107044=0
mmio 0x20107124=0
mmio 0x20108004=0
mmio 0x20107104=0

profile rom-rf-rx-saturation-reset
vendor-source rom
vendor-symbol phy_rfrx_sat_rst
rust-symbol open_phy_trace_rfrx_sat_rst
compare-return false

case disabled
arg 0
mmio 0x2010705c=0xffffffff

case enabled
arg 1
mmio 0x2010705c=0
mmio 0x201008bc=0
mmio 0x20107128=0

profile rom-rx-clock-enable
vendor-source rom
vendor-symbol phy_set_rxclk_en
rust-symbol open_phy_trace_set_rxclk_en
compare-return false

case disabled
arg 0
mmio 0x20100890=0xffffffff

case enabled
arg 1
mmio 0x20100890=0

profile rom-tx-clock-enable
vendor-source rom
vendor-symbol phy_set_txclk_en
rust-symbol open_phy_trace_set_txclk_en
compare-return false

case disabled
arg 0
mmio 0x20100890=0xffffffff

case enabled
arg 1
mmio 0x20100890=0

profile rom-nrx-frequency
vendor-source rom
vendor-symbol phy_nrx_freq_set
rust-symbol open_phy_trace_nrx_freq_set
compare-return false

case divide-by-zero
arg 0
mmio 0x20107848=0xa5000000

case ordinary-frequency
arg 2412
mmio 0x20107848=0

case signed-overflow
arg 0xffffffff
mmio 0x20107848=0x1b000000

case negative-divisor
arg 0xffffffff
mmio 0x20107848=0

profile rom-channel-cbw
vendor-source rom
vendor-symbol phy_bb_cbw_chan_cfg
rust-symbol open_phy_trace_bb_cbw_chan_cfg
compare-return false

case low-below-two
arg 0
mmio 0x20104400=0
mmio 0x20107ce0=0
mmio 0x20107ce4=0

case low-at-least-two
arg 2
mmio 0x20104400=0
mmio 0x20107ce0=0
mmio 0x20107ce4=0

case high-with-low-nibble
arg 0x53
mmio 0x20104400=0
mmio 0x20107ce0=0
mmio 0x20107ce4=0

case wrapping-high-domain
arg 0xffffffff
mmio 0x20104400=0
mmio 0x20107ce0=0
mmio 0x20107ce4=0

profile rom-i2c-tx-rate-init
vendor-source rom
vendor-symbol phy_i2c_txrate_init
rust-symbol open_phy_trace_i2c_txrate_init
compare-return false

case zeroed
mmio 0x2010448c=0
mmio 0x20100410=0
ram 0x2f07fc3c=0x3fff0000
vendor-ram-symbol 0x3fff0030=phy_txgain_comp_pacfg_new

case filled
mmio 0x2010448c=0xffffffff
mmio 0x20100410=0
ram 0x2f07fc3c=0x3fff0000
vendor-ram-symbol 0x3fff0030=phy_txgain_comp_pacfg_new

profile rom-pbus-debug-mode
vendor-source rom
vendor-symbol phy_pbus_debugmode
rust-symbol open_phy_trace_pbus_debugmode
compare-return false

case zeroed
mmio 0x2010088c=0
mmio 0x20100884=0

case filled
mmio 0x2010088c=0xffffffff
mmio 0x20100884=0xffffffff

profile rom-agc-register-init
vendor-source rom
vendor-symbol phy_agc_reg_init
rust-symbol open_phy_trace_agc_reg_init
compare-return false

case zero
arg 0
arg 0
mmio 0x2010713c=0
mmio 0x20107094=0
mmio 0x2010702c=0
mmio 0x2010705c=0
mmio 0x201008bc=0
mmio 0x20107128=0

case mixed-u8
arg 0x12
arg 0x34
mmio 0x2010713c=0
mmio 0x20107094=0
mmio 0x2010702c=0
mmio 0x2010705c=0
mmio 0x201008bc=0
mmio 0x20107128=0

case maximum-u8
arg 0xff
arg 0xff
mmio 0x2010713c=0
mmio 0x20107094=0
mmio 0x2010702c=0
mmio 0x2010705c=0
mmio 0x201008bc=0
mmio 0x20107128=0

profile rom-frequency-i2c-memory-write
vendor-source rom
vendor-symbol phy_freq_i2c_mem_write
rust-symbol open_phy_trace_freq_i2c_mem_write
compare-return false

case zero
arg 0
arg 0
arg 0
mmio 0x2010001c=0

case typical
arg 0x321
arg 0xabcdef
arg 0x55
mmio 0x2010001c=0

case full-vendor-input-domain
arg 0xffffffff
arg 0xffffffff
arg 0xffffffff
mmio 0x2010001c=0

profile archive-post-initialization-register-update
vendor-source archive
vendor-symbol phy_reg_update_new
rust-symbol open_phy_trace_phy_reg_update_new
compare-return false

case zeroed
mmio 0x2010705c=0
mmio 0x20107104=0
mmio 0x201078c8=0
mmio 0x20107d4c=0

case filled
mmio 0x2010705c=0xffffffff
mmio 0x20107104=0xffffffff
mmio 0x201078c8=0xffffffff
mmio 0x20107d4c=0xffffffff

profile archive-iccfr-enable
vendor-source archive
vendor-symbol phy_iccfr_en
rust-symbol open_phy_trace_phy_iccfr_en
compare-return false

case zero-selects-three
arg 0
mmio 0x2010747c=0

case nonzero-selects-zero
arg 1
mmio 0x2010747c=0xffffffff

profile archive-force-iccfr
vendor-source archive
vendor-symbol phy_force_iccfr
rust-symbol open_phy_trace_phy_force_iccfr
compare-return false

case zero
arg 0
arg 0
arg 0
mmio 0x20107478=0xffffffff
mmio 0x2010747c=0

case populated
arg 1
arg 1
arg 0x123
mmio 0x20107478=0
mmio 0x2010747c=0xffffffff

profile archive-dot11p-state
vendor-source archive
vendor-symbol phy_11p_set
rust-symbol open_phy_trace_phy_11p_set
contract state
compare-return false

case zero
arg 0
arg 0
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x28=2
rust-observe 0x3fff0000=2

case typical
arg 1
arg 0x5a
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x28=2
rust-observe 0x3fff0000=2

case low-bytes
arg 0xffffffff
arg 0x12345678
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x28=2
rust-observe 0x3fff0000=2

profile archive-current-level-state
vendor-source archive
vendor-symbol phy_current_level_set
rust-symbol open_phy_trace_phy_current_level_set
contract state
compare-return false

case zero
arg 0
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x2c=1
rust-observe 0x3fff0000=1

case low-byte
arg 0x12345678
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x2c=1
rust-observe 0x3fff0000=1

profile archive-bt-power-tracking-state
vendor-source archive
vendor-symbol phy_bt_power_track
rust-symbol open_phy_trace_phy_bt_power_track
contract state
compare-return false

case zero
arg 0
arg 0x3fff0000
rust-ram 0x3fff0000=1
vendor-observe-symbol phy_param+0xb=1
rust-observe 0x3fff0000=1

case low-byte
arg 0x12345678
arg 0x3fff0000
rust-ram 0x3fff0000=1
vendor-observe-symbol phy_param+0xb=1
rust-observe 0x3fff0000=1

profile archive-ble-channel-base-state
vendor-source archive
vendor-symbol phy_ble_set_chan_base
rust-symbol open_phy_trace_phy_ble_set_chan_base
contract state
compare-return false

case zero
arg 0
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x193=1
rust-observe 0x3fff0000=1

case low-byte
arg 0x12345678
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x193=1
rust-observe 0x3fff0000=1

profile archive-initialization-parameter-state
vendor-source archive
vendor-symbol phy_init_param_set
rust-symbol open_phy_trace_phy_init_param_set
contract state
compare-return false

case clear
arg 0
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x196=1
rust-observe 0x3fff0000=1

case low-bit-only
arg 0xffffffff
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x196=1
rust-observe 0x3fff0000=1

profile archive-temperature-tracking-debug-state
vendor-source archive
vendor-symbol phy_track_temp_debug
rust-symbol open_phy_trace_phy_track_temp_debug
contract state
compare-return false

case zero
arg 0
arg 0
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x1b0=2
rust-observe 0x3fff0000=2

case low-bytes
arg 0x12345678
arg 0xabcdef5a
arg 0x3fff0000
rust-ram 0x3fff0000=0
vendor-observe-symbol phy_param+0x1b0=2
rust-observe 0x3fff0000=2

profile archive-dc-value-split
vendor-source archive
vendor-symbol get_dc_value
rust-symbol open_phy_trace_get_dc_value
compare-return false

case zero
arg 0x3fff0000
arg 0
ram 0x3fff0000=0
observe 0x3fff0000=4

case halves
arg 0x3fff0000
arg 0x1234abcd
ram 0x3fff0000=0
observe 0x3fff0000=4

case full
arg 0x3fff0000
arg 0xffffffff
ram 0x3fff0000=0
observe 0x3fff0000=4

profile archive-i2c-master-memory-configuration
vendor-source archive
vendor-symbol phy_i2c_master_mem_cfg
rust-symbol open_phy_trace_phy_i2c_master_mem_cfg
compare-return false

case zero
arg 0x3fff0000
ram 0x3fff0000=0
ram 0x3fff0004=0
observe 0x3fff0000=6

case filled
arg 0x3fff0000
ram 0x3fff0000=0xffffffff
ram 0x3fff0004=0xffffffff
observe 0x3fff0000=6

profile archive-i2c-command-memory-configuration
vendor-source archive
vendor-symbol phy_i2c_master_command_mem_cfg
rust-symbol open_phy_trace_phy_i2c_master_command_mem_cfg
compare-return false

case zero
arg 0x3fff0000
arg 0x3fff0020
ram 0x3fff0000=0
ram 0x3fff0004=0
ram 0x3fff0020=0
observe 0x3fff0000=8
observe 0x3fff0020=4

case filled
arg 0x3fff0000
arg 0x3fff0020
ram 0x3fff0000=0xffffffff
ram 0x3fff0004=0xffffffff
ram 0x3fff0020=0xffffffff
observe 0x3fff0000=8
observe 0x3fff0020=4

profile archive-tx-attenuation-compensation
vendor-source archive
vendor-symbol phy_tx_atten_comp
rust-symbol open_phy_trace_phy_tx_atten_comp
compare-return false

case ordinary
arg 0x3fff0000
ram 0x3fff0000=0x00102030
observe 0x3fff0000=3

case wrapping
arg 0x3fff0000
ram 0x3fff0000=0x00ffff00
observe 0x3fff0000=3

profile rom-frequency-register-init
vendor-source rom
vendor-symbol phy_freq_reg_init
rust-symbol open_phy_trace_freq_reg_init
compare-return false

case default-parameters
arg 2
arg 4
arg 0
mmio 0x2010001c=0
ram 0x2f07fc40=0x3fff0000
ram 0x3fff0190=0

case parameter-override
arg 2
arg 4
arg 1
mmio 0x2010001c=0
ram 0x2f07fc40=0x3fff0000
ram 0x3fff0190=0x01000000

profile rom-disable-hardware-frequency
vendor-source rom
vendor-symbol phy_dis_hw_set_freq
rust-symbol open_phy_trace_dis_hw_set_freq
compare-return false

case zeroed
mmio 0x2010001c=0

case filled
mmio 0x2010001c=0xffffffff

profile rom-read-hardware-noise-floor
vendor-source rom
vendor-symbol phy_read_hw_noisefloor
rust-symbol open_phy_trace_ret_read_hw_noisefloor
compare-return true

case most-negative
mmio 0x2010708c=0

case typical
mmio 0x2010708c=0x00000a00

case minus-quarter
mmio 0x2010708c=0x00000fff

case masked-filled
mmio 0x2010708c=0xffffffff

profile archive-read-hardware-noise-floor
vendor-source archive
vendor-symbol read_hw_noisefloor
rust-symbol open_phy_trace_ret_read_hw_noisefloor
compare-return true

case most-negative
mmio 0x2010708c=0

case typical
mmio 0x2010708c=0x00000a00

case minus-quarter
mmio 0x2010708c=0x00000fff

case masked-filled
mmio 0x2010708c=0xffffffff

profile archive-antenna-diversity
vendor-source archive
vendor-symbol ant_dft_cfg
rust-symbol open_phy_trace_ant_dft_cfg
compare-return false

case disabled
arg 0
mmio 0x2010711c=0xffffffff

case enabled
arg 1
mmio 0x2010711c=0

case low-bit-only
arg 0xffffffff
mmio 0x2010711c=0x5a5a5a5a

profile rom-save-pbus-registers
vendor-source rom
vendor-symbol phy_save_pbus_reg
rust-symbol open_phy_trace_save_pbus_reg
compare-return false

case zeroed
arg 0x3fff0030
ram 0x2f07fc40=0x3fff0000
ram 0x3fff0030=0
ram 0x3fff0034=0
ram 0x3fff0038=0
ram 0x3fff003c=0
ram 0x3fff0040=0
ram 0x3fff0044=0
mmio 0x20100854=0
mmio 0x20100858=0
mmio 0x2010085c=0
mmio 0x20100860=0
mmio 0x20100864=0
mmio 0x20100868=0
observe 0x3fff0030=24

case patterned
arg 0x3fff0030
mmio 0x20100854=0x01234567
mmio 0x20100858=0x89abcdef
mmio 0x2010085c=0x13579bdf
mmio 0x20100860=0x2468ace0
mmio 0x20100864=0x55aa55aa
mmio 0x20100868=0xa55aa55a
ram 0x2f07fc40=0x3fff0000
ram 0x3fff0030=0
ram 0x3fff0034=0
ram 0x3fff0038=0
ram 0x3fff003c=0
ram 0x3fff0040=0
ram 0x3fff0044=0
observe 0x3fff0030=24

profile rom-absolute-temperature
vendor-source rom
vendor-symbol phy_abs_temp
rust-symbol open_phy_trace_ret_abs_temp
compare-return true

case zero
arg 0

case positive
arg 123

case negative
arg 0xffffff85

case minimum
arg 0x80000000

case maximum
arg 0x7fffffff

profile rom-encode-i2c-master
vendor-source rom
vendor-symbol phy_encode_i2c_master
rust-symbol open_phy_trace_ret_encode_i2c_master
compare-return true

case zero
arg 0
arg 0
arg 0

case typical
arg 0x67
arg 3
arg 0xab

case overlapping
arg 0xff000000
arg 0x00ff0000
arg 0x0000ff00

case full
arg 0xffffffff
arg 0xffffffff
arg 0xffffffff

profile rom-frequency-memory-address
vendor-source rom
vendor-symbol phy_get_freq_mem_addr
rust-symbol open_phy_trace_ret_get_freq_mem_addr
compare-return true

case zero
arg 0
arg 0
arg 0
arg 0

case typical
arg 0x20
arg 7
arg 84
arg 6

case multiply-wrap
arg 1
arg 0x80000000
arg 2
arg 3

case full
arg 0xffffffff
arg 0xffffffff
arg 0xffffffff
arg 0xffffffff

profile rom-byte-to-word
vendor-source rom
vendor-symbol phy_byte_to_word
rust-symbol open_phy_trace_ret_byte_to_word
compare-return true

case zero
arg 0x3fff0000
ram 0x3fff0000=0

case typical
arg 0x3fff0000
ram 0x3fff0000=0x5aab0367

case full
arg 0x3fff0000
ram 0x3fff0000=0xffffffff

profile rom-tx-power-tracking-slow-state
vendor-source rom
vendor-symbol phy_txpwr_track_slow
rust-symbol open_phy_trace_txpwr_track_slow
contract state
compare-return false

case zero
arg 0
arg 0x3fff0000
vendor-ram 0x2f07fc40=0x3fff0000
vendor-ram 0x3fff01ab=0
rust-ram 0x3fff0000=0
vendor-observe 0x3fff01ab=1
rust-observe 0x3fff0000=1

case typical
arg 0x55
arg 0x3fff0000
vendor-ram 0x2f07fc40=0x3fff0000
vendor-ram 0x3fff01ab=0
rust-ram 0x3fff0000=0
vendor-observe 0x3fff01ab=1
rust-observe 0x3fff0000=1

case low-byte
arg 0xffffffff
arg 0x3fff0000
vendor-ram 0x2f07fc40=0x3fff0000
vendor-ram 0x3fff01ab=0
rust-ram 0x3fff0000=0
vendor-observe 0x3fff01ab=1
rust-observe 0x3fff0000=1
