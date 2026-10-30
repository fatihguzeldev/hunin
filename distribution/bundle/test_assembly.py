import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from . import assembly as package

from ..installation import bundle as installer


RUNTIME_FILES = (
    "collectors/javascript/dist/plan.mjs",
    "collectors/javascript/dist/clock-preload.mjs",
    "collectors/javascript/dist/bun-preload.mjs",
    "collectors/javascript/dist/deno-workers.mjs",
    "collectors/javascript/dist/sources-preload.mjs",
    "collectors/javascript/dist/host/plan.mjs",
    "collectors/javascript/dist/agents/clock.mjs",
    "collectors/javascript/dist/agents/bun.mjs",
    "collectors/javascript/dist/agents/bun/manifest.mjs",
    "collectors/javascript/dist/agents/deno-workers.mjs",
    "collectors/javascript/dist/source/snapshots.mjs",
    "collectors/python/capture.py",
    "collectors/python/cpu.py",
    "collectors/python/cpu_clock.py",
    "collectors/python/decode.py",
    "collectors/python/entry.py",
    "collectors/python/graal.py",
    "collectors/python/hunin_capture/__init__.py",
    "collectors/python/hunin_capture/target.py",
    "collectors/python/hunin_capture/vmprof.py",
    "collectors/python/hunin_capture/graal.py",
    "collectors/python/hunin_capture/cpu_progress/__init__.py",
    "collectors/python/hunin_capture/cpu_progress/sampling.py",
    "collectors/python/hunin_capture/cpu_progress/clocks/__init__.py",
    "collectors/python/hunin_capture/cpu_progress/clocks/windows.py",
    "collectors/python/hunin_capture/cpu_progress/clocks/macos.py",
    "collectors/python/hunin_capture/cpu_progress/clocks/linux.py",
    "sdk/javascript/dist/index.mjs",
    "sdk/javascript/dist/index.d.mts",
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
    "install.py",
    "fetch_runtime.py",
    "runtime-assets.json",
    "bundle-assets.json",
)


class PackageTests(unittest.TestCase):
    def setUp(self):
        # Assembly fixtures use non-executable bytes; only the descriptor probe
        # is substituted. Its hashes still have to match every selected file.
        for owner in (package, installer):
            probe = patch.object(owner, "probe_binary", side_effect=self.product_info)
            probe.start()
            self.addCleanup(probe.stop)

    def product_info(self, binary):
        root = Path(binary).parent
        paths = installer.CONTRACT_FILES
        contracts = {
            name: {"sha256": package.sha256((root / path).read_bytes())}
            for name, path in paths.items()
        }
        for name in ("record", "responses"):
            contracts[name]["id"] = json.loads((root / paths[name]).read_bytes())["$id"]
        native = json.loads((root / paths["native_manifest"]).read_bytes())
        contracts["native_manifest"].update(
            {"version": native["version"], "extension": native["extension"]}
        )
        return {
            "schema_version": 1,
            "app": {
                "name": "hunin",
                "version": "0.1.0",
                "source_revision": None,
                "target": {"os": "linux", "arch": "x86_64"},
            },
            "contracts": contracts,
            "storage": {"maximum_schema_version": 2},
        }

    def test_existing_archive_or_checksum_is_not_overwritten(self):
        for reserved in ("archive", "checksum"):
            with (
                self.subTest(reserved=reserved),
                tempfile.TemporaryDirectory() as temporary,
            ):
                root = Path(temporary)
                binary = self.source_tree(root)
                output = root / "output"
                output.mkdir()
                archive = output / "hunin-0.1.0-linux-x64.tar.gz"
                path = (
                    archive
                    if reserved == "archive"
                    else archive.with_name(archive.name + ".sha256")
                )
                path.write_bytes(b"preserved accepted artifact")
                with self.assertRaisesRegex(ValueError, "Preserving existing"):
                    package.assemble(root, binary, output, "linux-x64")
                self.assertEqual(path.read_bytes(), b"preserved accepted artifact")

    def test_packaging_requires_the_executables_runtime_layout_binding(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = self.source_tree(root)
            old = self.product_info(binary)
            old["contracts"].pop("runtime_inventory", None)
            with (
                patch.object(package, "probe_binary", return_value=old),
                self.assertRaisesRegex(ValueError, "runtime inventory"),
            ):
                package.assemble(root, binary, root / "output", "linux-x64")

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
                    "patches": "patches/*.patch",
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
        inventory = json.loads(package.inventory.ASSETS.read_text())
        native = next(
            component
            for component in inventory["components"]
            if component["name"] == "native-collector"
        )
        for component in inventory["components"]:
            if component is native:
                continue
            for source in component["files"].values():
                path = root / component["source_root"] / source
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(str(path.relative_to(root)))
        (root / "bundle-assets.json").write_bytes(
            (root / "distribution/bundle/assets.json").read_bytes()
        )
        unselected = root / "distribution/unselected.py"
        unselected.parent.mkdir(parents=True, exist_ok=True)
        unselected.write_text("unselected development helper")
        native_root = Path(__file__).parents[2] / native["source_root"]
        patches = sorted(
            path.name for path in (native_root / "patches").glob("*.patch")
        )
        for name, data in self.native_files(patches).items():
            path = (
                root
                / native["source_root"]
                / Path(name).relative_to(native["destination_root"])
            )
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        ui = root / "ui/dist"
        ui.mkdir(parents=True, exist_ok=True)
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
        (root / "protocol/record.schema.json").write_text(
            json.dumps({"$id": "https://fixture/record-v1"})
        )
        (root / "protocol/response.schema.json").write_text(
            json.dumps({"$id": "https://fixture/response-v1"})
        )
        (root / "protocol/operations.json").write_text("[]")
        (root / "sdk/javascript/package.json").write_text(
            json.dumps({"version": "9.1.0"})
        )
        (root / "sdk/python/pyproject.toml").write_text('[project]\nversion="0.2.0"\n')
        (root / "sdk/rust/Cargo.toml").write_text('[package]\nversion="0.3.0"\n')
        binary = root / "hunin"
        binary.write_bytes(b"test-binary")
        return binary

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
            self.assertIsNone(manifest["product_build_info"]["app"]["source_revision"])
            self.assertEqual(
                manifest["sdk_versions"]["javascript"]["declared_version"], "9.1.0"
            )
            self.assertIsNone(manifest["sdk_versions"]["go"]["declared_version"])
            self.assertIn("collectors/python/cpu.py", manifest["files"])
            for name in RUNTIME_FILES:
                self.assertIn(name, manifest["files"])
            with patch.object(installer, "host_platform", return_value="linux-x64"):
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
                "collectors/javascript/dist/plan.test.mjs",
                "collectors/javascript/dist/test/plan.test.mjs",
                "collectors/javascript/src/host/plan.mts",
                "collectors/python/fixtures/raw.json",
                "collectors/native/test_build.py",
                "collectors/native/build.py",
                "sdk/javascript/src/index.mts",
                "sdk/javascript/dist/sdk.test.mjs",
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
            self.assertFalse("unselected.py" in files)

    def test_native_manifest_patch_cannot_be_omitted_from_runtime_inventory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = self.source_tree(root)
            # The existing binary receipt still matches every selected inventory
            # file. A newly selected source patch requires an explicit asset and
            # a rebuilt matching collector rather than silently shipping the old one.
            unselected = root / "collectors/native/patches/9999-required.patch"
            unselected.write_text("new build input deliberately absent from inventory")
            with self.assertRaisesRegex(ValueError, "Native patch inventory"):
                package.contents(root, binary, "linux-x64")

    def test_installed_vmprof_decoder_is_standalone_format_implementation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = self.source_tree(root)
            compatibility = root / "collectors/python/decode.py"
            compatibility.write_text("source-only compatibility entrypoint")
            implementation = root / "src/importers/vmprof/decode.py"
            files = package.contents(root, binary, "linux-x64")
            self.assertEqual(
                files["collectors/python/decode.py"], implementation.read_bytes()
            )
            self.assertNotEqual(
                files["collectors/python/decode.py"], compatibility.read_bytes()
            )

    def test_missing_emitted_runtime_asset_explains_which_build_is_required(self):
        for name in (
            "collectors/javascript/dist/agents/clock.mjs",
            "sdk/javascript/dist/index.d.mts",
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

    def test_binary_compiled_contract_mismatch_fails_before_creating_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = self.source_tree(root)
            stale = self.product_info(binary)
            stale["contracts"]["operations"]["sha256"] = "0" * 64
            with patch.object(package, "probe_binary", return_value=stale):
                with self.assertRaisesRegex(ValueError, "contract hash mismatch"):
                    package.assemble(root, binary, root / "output", "linux-x64")
            self.assertFalse((root / "output").exists())

    def test_binary_compiled_target_must_match_the_package_label(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = self.source_tree(root)
            wrong = self.product_info(binary)
            wrong["app"]["target"] = {"os": "macos", "arch": "aarch64"}
            with patch.object(package, "probe_binary", return_value=wrong):
                with self.assertRaisesRegex(ValueError, "compiled target"):
                    package.assemble(root, binary, root / "output", "linux-x64")
            del wrong["app"]["target"]
            with patch.object(package, "probe_binary", return_value=wrong):
                with self.assertRaisesRegex(ValueError, "no compiled target"):
                    package.assemble(root, binary, root / "output", "linux-x64")
            self.assertFalse((root / "output").exists())
