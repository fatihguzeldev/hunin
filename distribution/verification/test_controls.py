from pathlib import Path
import tempfile
import unittest

from . import installed as control
from . import native_sdk as native


class InstalledControlTests(unittest.TestCase):
    def test_native_sdk_search_follows_mixed_and_unaligned_pages(self):
        for operation, params, keys, first, second in [
            (
                "contexts.query",
                {"run_id": "recording"},
                ("contexts",),
                {"contexts": [], "relations": [{"id": "native-edge"}]},
                {"contexts": [{"id": "sdk-context"}]},
            ),
            (
                "timeline.query",
                {"run_id": "recording", "kind": "span", "alignment": "all"},
                ("records", "unaligned_records"),
                {"records": [{"id": "module"}]},
                {"records": [], "unaligned_records": [{"id": "sdk-span"}]},
            ),
            (
                "artifacts.list",
                {"run_id": "recording"},
                ("artifacts",),
                {"artifacts": [{"id": "image"}]},
                {"artifacts": [{"id": "sdk-jsonl"}]},
            ),
        ]:
            calls = []

            def call(name, request):
                calls.append((name, request))
                return (
                    dict(first, next_cursor="100")
                    if "cursor" not in request
                    else dict(second, next_cursor=None)
                )

            rows = list(native.paged_records(call, operation, params, keys))
            self.assertEqual(
                rows[-1]["id"],
                {
                    "contexts.query": "sdk-context",
                    "timeline.query": "sdk-span",
                    "artifacts.list": "sdk-jsonl",
                }[operation],
            )
            self.assertEqual(
                calls,
                [
                    (operation, dict(params, limit=100)),
                    (operation, dict(params, limit=100, cursor="100")),
                ],
            )
            self.assertNotIn("cursor", params)

    def test_native_sdk_search_reports_a_repeated_cursor(self):
        with self.assertRaisesRegex(RuntimeError, "repeated cursor"):
            list(
                native.paged_records(
                    lambda *_: {"contexts": [], "next_cursor": "100"},
                    "contexts.query",
                    {"run_id": "recording"},
                    ("contexts",),
                )
            )

    def test_adjacent_import_compares_decoded_file_identity_and_directory_boundary(
        self,
    ):
        with tempfile.TemporaryDirectory() as temporary:
            installed = Path(temporary) / "installed bundle %20"
            preload = installed / "collectors/javascript/clock-preload.mjs"
            preload.parent.mkdir(parents=True)
            preload.write_text("// installed control")
            self.assertTrue(
                control.adjacent_import(
                    f'--cpu-prof --import="{preload.as_uri()}"', installed
                )
            )
            self.assertTrue(
                control.adjacent_import(f'--import "{preload.as_uri()}"', installed)
            )
            outside = (
                installed.with_name(installed.name + "-other") / "clock-preload.mjs"
            )
            self.assertFalse(
                control.adjacent_import(f'--import="{outside.as_uri()}"', installed)
            )
            self.assertFalse(
                control.adjacent_import("--import=data:text/javascript,0", installed)
            )


if __name__ == "__main__":
    unittest.main()
