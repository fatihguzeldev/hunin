import importlib.util
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest


def load(name):
    spec = importlib.util.spec_from_file_location(
        name, Path(__file__).with_name(name + ".py")
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


package = load("package")
installer = load("install")

RUNTIME_SOURCES = (
    "collectors/javascript/plan.mjs",
    "collectors/javascript/clock-preload.mjs",
    "collectors/javascript/bun-preload.mjs",
    "collectors/javascript/deno-workers.mjs",
    "collectors/javascript/sources-preload.mjs",
    "collectors/javascript/host/plan.mjs",
    "collectors/javascript/agents/clock.mjs",
    "collectors/javascript/agents/bun.mjs",
    "collectors/javascript/agents/deno-workers.mjs",
    "collectors/javascript/source/snapshots.mjs",
    "collectors/python/capture.py",
    "collectors/python/cpu.py",
    "collectors/python/cpu_clock.py",
    "collectors/python/decode.py",
    "collectors/python/entry.py",
    "collectors/python/graal.py",
    "sdk/javascript/index.mjs",
    "sdk/javascript/index.d.mts",
    "sdk/javascript/package.json",
    "sdk/python/pyproject.toml",
    "sdk/python/hunin/__init__.py",
    "sdk/python/hunin/instrumentation.py",
    "sdk/rust/Cargo.toml",
    "sdk/rust/src/lib.rs",
    "sdk/rust/src/recording.rs",
    "sdk/go/go.mod",
    "sdk/go/hunin.go",
    "sdk/c/CMakeLists.txt",
    "sdk/c/include/hunin.h",
    "sdk/c/src/hunin.c",
    "sdk/cpp/include/hunin.hpp",
    "protocol/record.schema.json",
    "protocol/operations.json",
    "protocol/response.schema.json",
    "distribution/install.py",
    "distribution/fetch_runtime.py",
    "distribution/runtime-assets.json",
)


class PackageTests(unittest.TestCase):
    def native_files(self, patch_names=("0001.patch",)):
        base = "collectors/native/"
        upstream = {"revision": "test-pin", "archive_sha256": "test-archive"}
        extension = {"flag": "--lifetime-output", "schema_version": 1}
        patch_bytes = {
            name: ("test patch " + name + "\n").encode() for name in patch_names
        }
        patches = [
            {"name": name, "sha256": package.sha256(data)}
            for name, data in sorted(patch_bytes.items())
        ]
        binary = b"test-profiler"
        return {
            base + "manifest.json": json.dumps(
                {
                    "upstream": upstream,
                    "version": "test-version",
                    "extension": extension,
                }
            ).encode(),
            **{base + "patches/" + name: data for name, data in patch_bytes.items()},
            base + "bin/samply": binary,
            base + "bin/LICENSE-MIT": b"MIT test license",
            base + "bin/LICENSE-APACHE": b"Apache test license",
            base + "bin/samply.provenance.json": json.dumps(
                {
                    "target": "x86_64-unknown-linux-gnu",
                    "binary_sha256": package.sha256(binary),
                    "version": "test-version",
                    "extension": extension,
                    "patch_sha256": package.sha256(
                        json.dumps(
                            patches, sort_keys=True, separators=(",", ":")
                        ).encode()
                    ),
                    "identity": {"upstream": upstream, "patches": patches},
                }
            ).encode(),
        }

    def source_tree(self, root):
        """A disposable assembly fixture, not a built-product acceptance result."""
        (root / "Cargo.toml").write_text('[package]\nversion="0.1.0"\n')
        for name in (*RUNTIME_SOURCES, "distribution/fetch_samply.py"):
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        patches = sorted(
            path.name
            for path in (Path(__file__).parents[1] / "collectors/native/patches").glob(
                "*.patch"
            )
        )
        for name, data in self.native_files(patches).items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        ui = root / "ui/dist"
        ui.mkdir(parents=True)
        (ui / ".vite").mkdir()
        (ui / "assets").mkdir()
        (ui / "index.html").write_text(
            '<script type="module" src="/assets/main.js"></script>'
        )
        (ui / "hunin.svg").write_text("<svg></svg>")
        (ui / ".vite/manifest.json").write_text(
            json.dumps(
                {
                    "index.html": {
                        "file": "assets/main.js",
                        "isEntry": True,
                        "imports": ["_vendor.js"],
                        "dynamicImports": ["src/lazy.ts"],
                        "css": ["assets/main.css"],
                    },
                    "_vendor.js": {
                        "file": "assets/vendor.js",
                        "assets": ["assets/font.woff2"],
                    },
                    "src/lazy.ts": {"file": "assets/lazy.js"},
                }
            )
        )
        for name in ("main.js", "vendor.js", "lazy.js", "main.css", "font.woff2"):
            (ui / "assets" / name).write_text(name)
        binary = root / "hunin"
        binary.write_bytes(b"test-binary")
        return binary

    def bundle(self, root, version):
        root.mkdir(parents=True)
        files = {"hunin": b"test-binary", "ui/index.html": b"test-ui"}
        for name, data in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (root / "manifest.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "version": version,
                    "platform": "linux-x64",
                    "files": {
                        name: {
                            "size": len(data),
                            "sha256": hashlib.sha256(data).hexdigest(),
                        }
                        for name, data in files.items()
                    },
                }
            )
        )
        return root

    def test_bundle_retains_assets_and_install_verifies_integrity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = self.source_tree(root)
            archive = package.assemble(root, binary, root / "output", "linux-x64")
            import tarfile

            with tarfile.open(archive) as bundle:
                bundle.extractall(root / "extracted", filter="data")
            extracted = next((root / "extracted").iterdir())
            manifest = installer.verify(extracted)
            self.assertIn("collectors/python/cpu.py", manifest["files"])
            for name in RUNTIME_SOURCES:
                destination = name.removeprefix("distribution/")
                self.assertIn(destination, manifest["files"])
            installed = installer.install(extracted, root / "installed")
            self.assertEqual(installed.read_bytes(), b"test-binary")
            self.assertTrue((installed.parent / "ui/index.html").exists())
            if os.name != "nt":
                self.assertTrue(
                    os.access(
                        installed.parent / "collectors/native/bin/samply", os.X_OK
                    )
                )
            (extracted / "hunin").write_bytes(b"modified")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                installer.verify(extracted)

    def test_runtime_inventory_does_not_ship_new_tests_sources_or_build_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = self.source_tree(root)
            unshipped = (
                "collectors/javascript/plan.test.mjs",
                "collectors/javascript/test/plan.test.mjs",
                "collectors/javascript/src/host/plan.mts",
                "collectors/python/fixtures/raw.json",
                "collectors/native/test_build.py",
                "collectors/native/build.py",
                "sdk/javascript/src/index.mts",
                "sdk/javascript/sdk.test.mjs",
                "sdk/javascript/tsconfig.json",
                "sdk/rust/tests/concurrency.rs",
                "sdk/go/hunin_test.go",
                "sdk/c/tests/concurrency.cpp",
                "sdk/python/test_sdk.py",
                "sdk/conformance/javascript.mjs",
                "protocol/fixtures/test.json",
            )
            for name in unshipped:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("unrelated new source")
            files = package.contents(root, binary, "linux-x64")
            for name in unshipped:
                self.assertFalse(name in files, f"Unexpected runtime asset: {name}")
            self.assertFalse("fetch_samply.py" in files)

    def test_missing_emitted_runtime_asset_explains_which_build_is_required(self):
        for name in (
            "collectors/javascript/agents/clock.mjs",
            "sdk/javascript/index.d.mts",
            "ui/dist/.vite/manifest.json",
        ):
            with self.subTest(asset=name), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                binary = self.source_tree(root)
                (root / name).unlink()
                with self.assertRaisesRegex(ValueError, "npm.*build"):
                    package.contents(root, binary, "linux-x64")

    @unittest.skipIf(
        os.name == "nt", "Creating local symlinks requires host privileges"
    )
    def test_runtime_inventory_rejects_a_selected_source_symlink(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = self.source_tree(root)
            source = root / "sdk/c/src/hunin.c"
            source.unlink()
            outside = root / "unrelated-source"
            outside.write_text("unrelated private input")
            source.symlink_to(outside)
            with self.assertRaisesRegex(ValueError, "symlinks"):
                package.contents(root, binary, "linux-x64")

    def test_ui_manifest_closes_imports_without_shipping_unreferenced_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = self.source_tree(root)
            (root / "ui/dist/assets/unrelated-debug.js").write_text("unrelated")
            files = package.contents(root, binary, "linux-x64")
            for name in ("main.js", "vendor.js", "lazy.js", "main.css", "font.woff2"):
                self.assertIn("ui/assets/" + name, files)
            self.assertFalse("ui/assets/unrelated-debug.js" in files)

    def test_ui_manifest_cannot_select_paths_outside_the_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = self.source_tree(root)
            (root / "outside").write_text("unrelated private input")
            (root / "ui/dist/.vite/manifest.json").write_text(
                json.dumps({"index.html": {"isEntry": True, "file": "../../outside"}})
            )
            with self.assertRaisesRegex(ValueError, "[Uu]nsafe"):
                package.contents(root, binary, "linux-x64")

    def test_native_collector_must_match_the_shipped_platform_binary_and_patches(self):
        files = self.native_files()
        package.verify_native_collector(files, "linux-x64")
        for change in ("binary", "patch", "platform", "missing"):
            with self.subTest(change=change):
                changed = dict(files)
                target = "linux-x64"
                if change == "binary":
                    changed["collectors/native/bin/samply"] = b"different binary"
                elif change == "patch":
                    changed["collectors/native/patches/0001.patch"] = b"different patch"
                elif change == "platform":
                    target = "macos-arm64"
                else:
                    del changed["collectors/native/bin/samply"]
                with self.assertRaises(ValueError):
                    package.verify_native_collector(changed, target)

    def test_manifest_rejects_traversal(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "bundle").mkdir()
            (root / "outside").write_text("private")
            (root / "bundle/manifest.json").write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "version": "0.1.0",
                        "platform": "linux-x64",
                        "files": {"../outside": {"size": 7, "sha256": "unused"}},
                    }
                )
            )
            with self.assertRaisesRegex(ValueError, "Unsafe package path"):
                installer.verify(root / "bundle")

    @unittest.skipIf(os.name == "nt", "The optional bin symlink is Unix-only")
    def test_bin_link_preserves_unrelated_and_dangling_targets(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.bundle(root / "bundle", "0.2.0")
            for kind in ("unrelated", "dangling"):
                with self.subTest(kind=kind):
                    case = root / kind
                    (case / "bin").mkdir(parents=True)
                    target = case / "another-program"
                    if kind == "unrelated":
                        target.write_bytes(b"unrelated executable")
                    link = case / "bin/hunin"
                    link.symlink_to(target)
                    with self.assertRaisesRegex(
                        ValueError, "Preserving existing executable"
                    ):
                        installer.install(bundle, case / "installed", case / "bin")
                    self.assertEqual(link.readlink(), target)
                    if kind == "unrelated":
                        self.assertEqual(target.read_bytes(), b"unrelated executable")

    @unittest.skipIf(os.name == "nt", "The optional bin symlink is Unix-only")
    def test_bin_link_upgrades_only_a_verified_installation_under_its_prefix(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            prefix = root / "installed"
            previous = self.bundle(prefix / "0.1.0-linux-x64", "0.1.0")
            bundle = self.bundle(root / "bundle", "0.2.0")
            (root / "bin").mkdir()
            link = root / "bin/hunin"
            link.symlink_to(previous / "hunin")
            executable = installer.install(bundle, prefix, root / "bin")
            self.assertEqual(link.resolve(), executable.resolve())
            self.assertTrue((previous / "hunin").is_file())
            # A valid-looking directory name does not establish ownership if its bytes changed.
            (previous / "hunin").write_bytes(b"modified")
            link.unlink()
            link.symlink_to(previous / "hunin")
            self.assertFalse(installer.owned_link(link, prefix))


if __name__ == "__main__":
    unittest.main()
