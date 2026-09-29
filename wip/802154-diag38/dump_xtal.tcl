proc rd {a} { return [lindex [read_memory $a 32 1] 0] }
init
halt
set conf2 0x2010F820
set saved2 [rd $conf2]
if {(($saved2 >> 4) & 0x3fff) != 0x3fa0} { mww $conf2 [expr {($saved2 & ~(0x3fff << 4)) | (0x3fa0 << 4)}] }
set ctrl 0x2010F804
foreach r {0x09 0x0a} {
    for {set i 0} {$i < 1000} {incr i} { if {!([rd $ctrl] & 0x02000000)} break }
    mww 0x2010F81C 0xfffffeff
    mww $ctrl [expr {0x61 | ($r << 8)}]
    for {set i 0} {$i < 1000} {incr i} { set v [rd $ctrl]; if {!($v & 0x02000000)} break }
    echo [format "I2C 61 %02x %02x" $r [expr {($v >> 16) & 0xff}]]
}
mww $conf2 $saved2
resume
shutdown
