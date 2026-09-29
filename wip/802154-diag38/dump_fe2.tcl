proc rd {a} { return [lindex [read_memory $a 32 1] 0] }
set skip {0x201070cc 0x20108004 0x20108050 0x20108078}
proc skipped {a} {
    global skip
    if {$a >= 0x20102fa0 && $a < 0x20102fc0} { return 1 }
    foreach s $skip { if {$a == $s} { return 1 } }
    return 0
}
proc dump_words {start end} {
    for {set a $start} {$a < $end} {incr a 4} {
        if {[skipped $a]} { echo [format "W %08x SKIP" $a]; continue }
        if {[catch {set v [rd $a]} err]} { echo [format "W %08x ERR" $a]; continue }
        echo [format "W %08x %08x" $a $v]
    }
}
proc dump_chunks {start end} {
    for {set a $start} {$a < $end} {incr a 0x100} {
        set e [expr {$a + 0x100}]
        if {$e > $end} { set e $end }
        set bad 0
        for {set b $a} {$b < $e} {incr b 4} { if {[skipped $b]} { set bad 1 } }
        if {$bad} { dump_words $a $e; continue }
        set n [expr {($e - $a) / 4}]
        if {[catch {set words [read_memory $a 32 $n]} err]} { dump_words $a $e; continue }
        for {set i 0} {$i < $n} {incr i} { echo [format "W %08x %08x" [expr {$a + 4*$i}] [lindex $words $i]] }
    }
}
init
halt
set conf2 0x2010F820
set saved2 [rd $conf2]
if {(($saved2 >> 4) & 0x3fff) != 0x3fa0} { mww $conf2 [expr {($saved2 & ~(0x3fff << 4)) | (0x3fa0 << 4)}] }
foreach {block host low} {0x61 1 0x00fffeff 0x62 1 0x00ffffdf 0x63 1 0x00ffffef 0x66 0 0x00ffff7f 0x67 1 0x00fffffb 0x69 0 0x00fff7ff 0x6a 1 0x00ffffbf 0x6b 1 0x00fffff7 0x6d 0 0x00ff7fff} {
    set ctrl [expr {0x2010F800 + 4*$host}]
    for {set r 0} {$r < 32} {incr r} {
        for {set i 0} {$i < 1000} {incr i} { if {!([rd $ctrl] & 0x02000000)} break }
        mww 0x2010F81C [expr {0xff000000 | $low}]
        mww $ctrl [expr {$block | ($r << 8)}]
        for {set i 0} {$i < 1000} {incr i} { set v [rd $ctrl]; if {!($v & 0x02000000)} break }
        echo [format "I2C %02x %02x %02x" $block $r [expr {($v >> 16) & 0xff}]]
    }
}
mww $conf2 $saved2
dump_chunks 0x20106000 0x20108000
resume
shutdown
