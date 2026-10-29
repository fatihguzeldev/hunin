import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "package", Path(__file__).with_name("package.py")
)
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class BuildMetadataTests(unittest.TestCase):
    def test_records_actual_host_and_explicit_build_provenance(self):
        def output(command):
            if "rev-parse" in command:
                return "abc123"
            if "status" in command:
                return " M source.rs"
            self.assertEqual(command, ["rustc", "+stable", "--version", "--verbose"])
            return "rustc 1.90.0 (abc)\nhost: x86_64-unknown-linux-gnu\nrelease: 1.90.0"

        with (
            patch.object(package, "command_output", side_effect=output),
            patch.object(package.platform, "system", return_value="Linux"),
            patch.object(package.platform, "release", return_value="6.8.0"),
            patch.object(
                package.platform, "version", return_value="tested kernel build"
            ),
            patch.object(package.platform, "libc_ver", return_value=("glibc", "2.39")),
            patch.object(
                package.platform,
                "freedesktop_os_release",
                return_value={"PRETTY_NAME": "Ubuntu 24.04"},
            ),
            patch.dict(
                package.os.environ,
                {
                    "GITHUB_SERVER_URL": "https://github.com",
                    "GITHUB_REPOSITORY": "example/hunin",
                    "GITHUB_RUN_ID": "123",
                    "GITHUB_SHA": "def456",
                },
                clear=True,
            ),
        ):
            metadata = package.build_metadata(
                Path("/source"), "stable", "x86_64-unknown-linux-gnu"
            )
        self.assertEqual(metadata["built_on"]["release"], "6.8.0")
        self.assertEqual(
            metadata["built_on"]["libc"], {"name": "glibc", "version": "2.39"}
        )
        self.assertEqual(metadata["built_on"]["distribution"], "Ubuntu 24.04")
        self.assertEqual(metadata["build_provenance"]["source_revision"], "abc123")
        self.assertTrue(metadata["build_provenance"]["source_dirty"])
        self.assertEqual(metadata["build_provenance"]["github_sha"], "def456")
        self.assertEqual(
            metadata["build_provenance"]["workflow_run_url"],
            "https://github.com/example/hunin/actions/runs/123",
        )
        self.assertEqual(metadata["build_provenance"]["rust"]["version"], "1.90.0")
        self.assertEqual(
            metadata["build_provenance"]["rust"]["target"], "x86_64-unknown-linux-gnu"
        )
        self.assertEqual(metadata["compatibility"]["minimum_os"], "not_established")

    def test_missing_provenance_remains_unknown_instead_of_claiming_a_build(self):
        with (
            patch.object(package, "command_output", return_value=None),
            patch.dict(package.os.environ, {}, clear=True),
        ):
            metadata = package.build_metadata(Path("/unversioned"))
        provenance = metadata["build_provenance"]
        self.assertEqual(provenance["source_revision"], "unknown")
        self.assertEqual(provenance["source_dirty"], "unknown")
        self.assertEqual(provenance["github_sha"], "unknown")
        self.assertEqual(provenance["workflow_run_url"], "unknown")
        self.assertEqual(set(provenance["rust"].values()), {"unknown"})


if __name__ == "__main__":
    unittest.main()
