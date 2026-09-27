# Analog register bus

`oer-radio-analog` is the chip-neutral contract the Espressif PHY uses to
reach its analog blocks: byte registers behind an analog I2C master. Where
the vendor leaves `phy_chip_i2c_readReg` and `phy_chip_i2c_writeReg`
busy-wait, `AnalogRegisterBus` splits each command into a start and an
observed completion, so the executor that polls it owns every wait and
deadline.

A chip's HAL implements the trait over its PAC. The address type is the
chip's own: host selection, read masks and block aliases stay behind it, and
completion is always observed with the address that started the command.
`FieldRead` and `FieldWrite` are the vendor field transactions
`phy_i2c_readReg_Mask` and `phy_i2c_writeReg_Mask` as polled machines; a
field write reads the register, replaces bits `msb..=lsb` and writes the
byte back. The vendor ORs an unmasked value into the byte, so `FieldWrite`
rejects a value wider than its field instead of reproducing that leak.

`ParallelWrites` is `phy_i2c_paral_write_num` over `ParallelAnalogBus`: it
installs a parallel host map, publishes each pair of host commands, polls
host 0 and then host 1 idle, and restores the normal map.

The ESP32-C5 implementation is `oer_esp32c5_hal::analog::AnalogI2c`.
