init
halt
foreach {name start count} [list PARAM $PARAM 124 ADDR $ADDR 1] {
    set words [read_memory $start 32 $count]
    for {set i 0} {$i < $count} {incr i} { echo [format "%s 0x%03x 0x%08x" $name [expr {4*$i}] [lindex $words $i]] }
}
set p [lindex [read_memory $ADDR 32 1] 0]
if {$p >= 0x2f000000 && $p < 0x30000000} {
    set words [read_memory $p 32 124]
    for {set i 0} {$i < 124} {incr i} { echo [format "PTR 0x%03x 0x%08x" [expr {4*$i}] [lindex $words $i]] }
}
resume
shutdown
