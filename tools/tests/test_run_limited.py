"""Exercise watchdog failure and shutdown with tiny local processes."""

import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import unittest


WRAPPER = Path(__file__).resolve().parents[1] / "blobray/scripts/run-limited"


@unittest.skipUnless(sys.platform == "linux", "the watchdog requires Linux /proc")
class RunLimitedTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="oer-watchdog-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.ready = self.root / "ready"
        self.binary = self.root / "command"
        self.write_executable(self.binary, f"""#!{sys.executable}
import os
import signal
import time
from pathlib import Path
if os.environ.get("COMMAND_MODE") == "exit":
    raise SystemExit(23)
signal.signal(signal.SIGTERM, signal.SIG_IGN)
ready = Path(os.environ["READY"])
pending = ready.with_suffix(".pending")
pending.write_text(str(os.getpid()))
pending.rename(ready)
while True:
    time.sleep(1)
""")
        real_ps = shutil.which("ps")
        self.assertIsNotNone(real_ps)
        self.write_executable(self.root / "ps", f"""#!/usr/bin/env bash
if [[ -f "$READY" ]]; then
    case "$PS_MODE" in
        fail) exit 7 ;;
        rss-fail) [[ "$*" != *rss* ]] || exit 7 ;;
        malformed) echo 'not a process table'; exit 0 ;;
        empty) exit 0 ;;
    esac
fi
exec {real_ps} "$@"
""")

    @staticmethod
    def write_executable(path, content):
        path.write_text(content)
        path.chmod(0o755)

    def start(self, ps_mode="normal", command_mode="hold"):
        process = subprocess.Popen(
            ["bash", str(WRAPPER)],
            env={
                **os.environ,
                "PATH": str(self.root) + os.pathsep + os.environ["PATH"],
                "BLOBRAY_BINARY": str(self.binary),
                "BLOBRAY_LIMIT_BACKEND": "watchdog",
                "BLOBRAY_REPORT_USAGE": "1",
                "READY": str(self.ready),
                "PS_MODE": ps_mode,
                "COMMAND_MODE": command_mode,
            },
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        self.addCleanup(self.stop, process)
        return process

    def stop(self, process):
        # The command has its own session. Always clean it up, including when
        # exercising the original broken wrapper and an assertion times out.
        if self.ready.exists():
            try:
                os.killpg(int(self.ready.read_text()), signal.SIGKILL)
            except ProcessLookupError:
                pass
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
        process.communicate(timeout=10)

    def wait_ready(self, process):
        deadline = time.monotonic() + 10
        while not self.ready.exists() and time.monotonic() < deadline:
            if process.poll() is not None:
                self.fail("wrapper exited before the command started")
            time.sleep(0.01)
        self.assertTrue(self.ready.exists(), "command did not start")

    def assert_command_stopped(self):
        if self.ready.exists():
            with self.assertRaises(ProcessLookupError):
                os.kill(int(self.ready.read_text()), 0)

    def test_command_exit_status_is_preserved(self):
        process = self.start(command_mode="exit")
        _, stderr = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 23, stderr)
        self.assertIn("blobray usage:", stderr)

    def test_process_enumeration_failure_stops_the_command(self):
        process = self.start(ps_mode="fail")
        _, stderr = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 137, stderr)
        self.assertIn("could not inspect", stderr)
        self.assert_command_stopped()

    def test_rss_query_failure_cannot_be_treated_as_zero_usage(self):
        process = self.start(ps_mode="rss-fail")
        _, stderr = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 137, stderr)
        self.assertIn("could not inspect", stderr)
        self.assert_command_stopped()

    def test_malformed_process_output_stops_the_command(self):
        process = self.start(ps_mode="malformed")
        _, stderr = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 137, stderr)
        self.assertIn("could not inspect", stderr)
        self.assert_command_stopped()

    def test_empty_process_table_cannot_hide_the_live_command(self):
        # An exited command may be absent, but ps -e must still see itself and
        # this wrapper. Entirely missing telemetry is not a zero-RSS sample.
        process = self.start(ps_mode="empty")
        _, stderr = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 137, stderr)
        self.assertIn("could not inspect", stderr)
        self.assert_command_stopped()

    def test_term_escalates_when_the_command_ignores_it(self):
        process = self.start()
        self.wait_ready(process)
        process.send_signal(signal.SIGTERM)
        # Keep the production ten-second grace period; allow ample scheduling
        # margin without running an analysis or changing configured limits.
        _, stderr = process.communicate(timeout=25)
        self.assertEqual(process.returncode, 137, stderr)
        self.assertIn("received TERM", stderr)
        self.assert_command_stopped()


if __name__ == "__main__":
    unittest.main()
