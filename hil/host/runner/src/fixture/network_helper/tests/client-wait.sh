# Load the actual helper's functions through its read-only capabilities action.
# Bash permits substituting its absolute command names without touching /usr/bin.
set -- capabilities
. "$OER_TEST_HELPER"

function /usr/bin/wpa_cli {
    case "$OER_TEST_CLIENT" in
        connected) echo wpa_state=COMPLETED;;
        timeout) echo wpa_state=SCANNING;;
        control-error) echo 'injected control socket failure' >&2; return 7;;
        malformed) echo FAIL;;
    esac
}
function /bin/sleep { :; }
function /usr/bin/iw { :; }
stop_radio_services() { :; }

wait_client_association
