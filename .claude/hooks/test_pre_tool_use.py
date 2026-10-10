"""Regressions of pre_tool_use.py, run as Claude Code runs it: JSON on stdin.

`cargo xtask check` runs them (check `claude-hooks`) when `.claude/hooks/`
changes; by hand: `python3 -m unittest discover -s .claude/hooks`.
"""

import json
import os
import subprocess
import sys
import unittest

HOOK = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pre_tool_use.py")


def run(tool, **tool_input):
    """The hook's exit status and stderr for one tool call."""
    event = {"tool_name": tool, "tool_input": tool_input}
    result = subprocess.run([sys.executable, HOOK], input=json.dumps(event),
                            capture_output=True, text=True, check=False)
    return result.returncode, result.stderr


class Edits(unittest.TestCase):
    def assert_blocked(self, tool, path, command):
        status, stderr = run(tool, file_path=path)
        self.assertEqual(status, 2, path)
        self.assertIn("is generated", stderr)
        self.assertIn(command, stderr)

    def test_generated_files_are_regenerated_not_edited(self):
        for tool, path, command in [
            ("Edit", "/w/crates/hardware/esp32s31/pac/src/generated.rs", "cargo registers generate"),
            ("Write", "crates/hardware/esp32c5/pac/raw/src/lib.rs", "cargo registers generate"),
            ("Edit", "/w/registers/esp32s31/published/radio.svd", "cargo registers generate"),
            ("Edit", "registers/esp32s31/published/platform.bindings.toml", "cargo registers generate"),
            ("Edit", "/w/verification/esp32s31/facts/provenance.toml", "--accept"),
            ("Write", "verification/esp32s31/facts/names/libble_app.toml", "oer-symbol-lineage"),
            ("Edit", "/w/.claude/hooks/heavy-commands.json", "cargo xtask hooks"),
        ]:
            self.assert_blocked(tool, path, command)

    def test_handwritten_neighbours_stay_editable(self):
        for path in [
            "/w/crates/hardware/esp32s31/pac/src/lib.rs",
            "/w/registers/esp32s31/model/device.toml",
            "/w/verification/esp32s31/facts/names/README.md",
            "/w/verification/esp32s31/artifacts.toml",
            "/w/.claude/hooks/pre_tool_use.py",
        ]:
            self.assertEqual(run("Edit", file_path=path), (0, ""), path)


class Reads(unittest.TestCase):
    def test_bash_never_prints_an_observer_json_whole(self):
        status, stderr = run("Bash", command="cat target/hil/runs/r1/observers/station.json | head")
        self.assertEqual(status, 2)
        self.assertIn("cargo hil runs why", stderr)

    def test_bash_still_refuses_whole_generated_reads(self):
        status, stderr = run("Bash", command="head -n 50 registers/esp32s31/published/radio.svd")
        self.assertEqual(status, 2)
        self.assertIn("grep it with an explicit path", stderr)

    def test_listing_observers_is_allowed(self):
        self.assertEqual(run("Bash", command="ls target/hil/runs/r1/observers/"), (0, ""))


class Commands(unittest.TestCase):
    def test_foreground_builds_are_refused_and_background_ones_run(self):
        status, stderr = run("Bash", command="cargo test -p oer-tidy")
        self.assertEqual(status, 2)
        self.assertIn("run_in_background", stderr)
        self.assertEqual(run("Bash", command="cargo test -p oer-tidy",
                             run_in_background=True), (0, ""))

    def test_waiting_by_process_name_is_refused(self):
        status, _ = run("Bash", command="pgrep -f 'cargo hil'")
        self.assertEqual(status, 2)


if __name__ == "__main__":
    unittest.main()
