import unittest

from . import native_sdk as native


class NativeSdkControlTests(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
