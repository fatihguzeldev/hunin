import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

if __package__.startswith("distribution."):
    from .. import fetch_samply as legacy
else:
    import fetch_samply as legacy


class RetiredStockNativeInstallerTests(unittest.TestCase):
    def test_stock_native_installer_fails_before_network_or_destination_creation(self):
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "tool"
            with patch(
                "urllib.request.urlopen",
                side_effect=AssertionError("unexpected network"),
            ) as request:
                with self.assertRaisesRegex(ValueError, "Hunin.*pinned"):
                    legacy.install("macos-arm64", destination)
                request.assert_not_called()
            self.assertFalse(destination.exists())

    def test_legacy_command_explains_the_pinned_build_without_changing_existing_data(
        self,
    ):
        with tempfile.TemporaryDirectory() as temporary:
            work = Path(temporary)
            destination = work / "existing-tool"
            destination.mkdir()
            sentinel = destination / "preserved"
            sentinel.write_text("existing data")
            environment = work / "github-env"
            environment.write_text("existing environment\n")
            result = subprocess.run(
                [
                    sys.executable,
                    legacy.__file__,
                    "--platform",
                    "macos-arm64",
                    "--destination",
                    str(destination),
                    "--github-env",
                ],
                cwd=work,
                capture_output=True,
                text=True,
                env={**os.environ, "GITHUB_ENV": str(environment)},
            )
            self.assertEqual(result.returncode, 2)
            self.assertIn("collectors/native/build.py", result.stderr)
            self.assertEqual(result.stdout, "")
            self.assertEqual(sentinel.read_text(), "existing data")
            self.assertEqual(environment.read_text(), "existing environment\n")
