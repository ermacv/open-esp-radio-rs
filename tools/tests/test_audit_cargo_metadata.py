"""Exercise manifest discovery during an unstaged workspace relocation."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


AUDIT = Path(__file__).resolve().parents[1] / "audit-cargo-metadata.sh"


class MetadataAuditTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="oer-metadata-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "tools").mkdir()
        shutil.copy2(AUDIT, self.root / "tools" / AUDIT.name)
        self.run_command("git", "init", "--quiet")
        self.write(".gitignore", "/_oracles/\n**/target/\n")
        self.package(".", "root_fixture")
        self.package("driver/old", "independent_fixture")
        self.run_command("git", "add", ".")
        (self.root / "driver/old").rename(self.root / "driver/moved")

    def run_command(self, *args, check=True):
        return subprocess.run(
            args, cwd=self.root, text=True, capture_output=True, check=check,
            env={**os.environ, "CARGO_NET_OFFLINE": "true"},
        )

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def package(self, directory, name):
        manifest = str(Path(directory) / "Cargo.toml")
        self.write(manifest, f'[package]\nname = "{name}"\nversion = "0.1.0"\n'
                   'edition = "2024"\n\n[workspace]\n')
        self.write(str(Path(directory) / "src/lib.rs"), "")
        self.run_command("cargo", "generate-lockfile", "--manifest-path", manifest)

    def test_unstaged_move_is_checked_without_private_or_build_inputs(self):
        # A force-added private manifest must remain outside the source audit.
        self.write("_oracles/private/Cargo.toml", "invalid private manifest")
        self.run_command("git", "add", "--force", "_oracles/private/Cargo.toml")
        self.write("driver/moved/target/local/Cargo.toml", "invalid build manifest")

        result = self.run_command("bash", "tools/audit-cargo-metadata.sh")

        self.assertIn("checking locked Cargo metadata: driver/moved/Cargo.toml", result.stdout)
        self.assertIn("passed for 2 workspace(s)", result.stdout)
        self.assertNotIn("driver/old", result.stdout)

    def test_invalid_unstaged_workspace_fails_the_audit(self):
        self.write("driver/moved/Cargo.toml", "invalid new manifest")

        result = self.run_command("bash", "tools/audit-cargo-metadata.sh", check=False)

        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("locked Cargo metadata passed", result.stdout)


if __name__ == "__main__":
    unittest.main()
