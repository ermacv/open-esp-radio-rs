# Command substitutes for the fixture owner's real remote programs.
nft() { :; }
kill() { :; }
rm() {
    case "$*" in *open-radio-client*) return 0;; esac
    command rm "$@"
}
wifi() {
    if test "$1" = up && test "$OER_TEST_CASE" = prepare-cleanup-error; then
        echo 'injected wireless recovery failure' >&2; return 1
    fi
    printf %s "$1" > "$OER_TEST_STATE/wireless"
}
sleep() {
    case "$OER_TEST_CASE" in
        prepare-error|prepare-cleanup-error) return 1;;
        prepare-cancel) touch "$OER_TEST_STATE/ready"; command sleep 20;;
    esac
}
iw() {
    case "$*" in
        'dev phy0-ap0 info')
            if test "$OER_TEST_CASE" = prepare-wrong-channel; then
                printf 'wiphy 0\nchannel 13 (2472 MHz), width: 40 MHz\n'
            else
                printf 'wiphy 0\nchannel 6 (2437 MHz), width: 40 MHz\n'
            fi;;
        'dev open-radio-mon info') test -f "$OER_TEST_STATE/monitor";;
        'dev open-radio-mon del') command rm -f "$OER_TEST_STATE/monitor";;
        'phy phy0 interface add open-radio-mon type monitor') echo owned > "$OER_TEST_STATE/monitor";;
        'dev or-ap-client del') :;;
        *) echo "unexpected iw: $*" >&2; return 1;;
    esac
}
ip() {
    case "$*" in
        'neigh show '*) echo '192.0.2.2 lladdr 02:00:00:00:00:01 REACHABLE';;
        'link set open-radio-mon up')
            case "$OER_TEST_CASE" in
                monitor-error) return 1;;
                monitor-cancel) touch "$OER_TEST_STATE/ready"; command sleep 20;;
            esac;;
        *) return 1;;
    esac
}
mkdir() {
    printf '%s\n' "$1" > "$OER_TEST_STATE/remote-directory"
    command mkdir "$@"
}
timeout() { command sleep 20; }
