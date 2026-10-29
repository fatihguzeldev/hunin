import json
from pathlib import Path
import py_compile
import subprocess
import sys
import tempfile
import textwrap
import unittest
import zipfile


HERE = Path(__file__).resolve().parent
RUN_TARGET = (
    "import sys; "
    f"sys.path.insert(0, {str(HERE)!r}); "
    "from hunin_capture.target import run_target; "
    "run_target(sys.argv[1:])"
)


class TargetEntryTest(unittest.TestCase):
    maxDiff = None

    def invoke(self, arguments, work, *, capture):
        return subprocess.run(
            [sys.executable, *(["-c", RUN_TARGET] if capture else []), *arguments],
            cwd=work,
            capture_output=True,
            text=True,
            timeout=30,
        )

    def test_target_main_identity_argv_and_file_match_the_selected_interpreter(self):
        source = textwrap.dedent("""\
            import __main__, json, sys
            marker = 'defined by target'
            print(json.dumps({
                'marker': __main__.marker,
                'same_globals': globals() is vars(__main__),
                'argv': sys.argv,
                'file': globals().get('__file__'),
                'spec': __spec__.name if __spec__ else None,
            }, sort_keys=True))
            """)
        with tempfile.TemporaryDirectory(prefix="hunin-python-entry-") as directory:
            work = Path(directory)
            script = work / "main.py"
            script.write_text(source)
            package = work / "target_package"
            package.mkdir()
            (package / "__init__.py").write_text("")
            (package / "main.py").write_text(source)
            for target in (
                [str(script)],
                ["-m", "target_package.main"],
                ["-c", source],
            ):
                with self.subTest(target=target[:2]):
                    arguments = [*target, "argument with spaces"]
                    direct = self.invoke(arguments, work, capture=False)
                    captured = self.invoke(arguments, work, capture=True)
                    self.assertEqual(direct.returncode, 0, direct.stderr)
                    self.assertEqual(captured.returncode, 0, captured.stderr)
                    self.assertEqual(
                        json.loads(captured.stdout), json.loads(direct.stdout)
                    )

    def test_command_globals_match_the_selected_interpreter(self):
        source = textwrap.dedent("""\
            import json
            print(json.dumps({
                'package': __package__,
                'file': globals().get('__file__'),
                'cached': globals().get('__cached__'),
                'spec': __spec__,
                'annotations': globals().get('__annotations__'),
                'loader': getattr(__loader__, '__name__', None),
            }, sort_keys=True))
            """)
        with tempfile.TemporaryDirectory(prefix="hunin-python-globals-") as directory:
            direct = self.invoke(["-c", source], directory, capture=False)
            captured = self.invoke(["-c", source], directory, capture=True)
            self.assertEqual(direct.returncode, 0, direct.stderr)
            self.assertEqual(captured.returncode, 0, captured.stderr)
            self.assertEqual(json.loads(captured.stdout), json.loads(direct.stdout))

    def test_command_namespace_remains_available_to_exit_callbacks(self):
        source = textwrap.dedent("""\
            import atexit
            marker = 'target still available'
            def on_exit():
                import __main__
                print(__main__.marker)
            atexit.register(on_exit)
            raise SystemExit(7)
            """)
        with tempfile.TemporaryDirectory(prefix="hunin-python-exit-") as directory:
            direct = self.invoke(["-c", source], directory, capture=False)
            captured = self.invoke(["-c", source], directory, capture=True)
            self.assertEqual(direct.returncode, 7)
            self.assertEqual(captured.returncode, direct.returncode)
            self.assertEqual(captured.stdout, direct.stdout)
            self.assertEqual(captured.stderr, direct.stderr)

    def test_script_and_module_keep_native_main_globals_through_exit(self):
        source = textwrap.dedent("""\
            import atexit, json, os, sys
            marker = 'target still available'
            def observe(stage):
                import __main__
                exec('nested_result = 42', globals())
                loader = globals().get('__loader__')
                print(json.dumps({
                    'stage': stage,
                    'same_globals': globals() is vars(__main__),
                    'marker': getattr(__main__, 'marker', None),
                    'nested_result': getattr(__main__, 'nested_result', None),
                    'argv': sys.argv,
                    'cwd': os.getcwd(),
                    'path0': sys.path[0],
                    'package': globals().get('__package__'),
                    'file': globals().get('__file__'),
                    'cached': globals().get('__cached__'),
                    'spec': __spec__.name if __spec__ else None,
                    'loader': getattr(loader, '__name__', type(loader).__name__),
                    'loader_path': getattr(loader, 'path', None),
                    'builtins_type': type(__builtins__).__name__,
                }, sort_keys=True))
            atexit.register(observe, 'atexit')
            observe('target')
            raise SystemExit(7)
            """)
        with tempfile.TemporaryDirectory(prefix="hunin-python-main-exit-") as directory:
            work = Path(directory)
            (work / "main.py").write_text(source)
            package = work / "target_package"
            package.mkdir()
            (package / "__init__.py").write_text("")
            (package / "main.py").write_text(source)
            (package / "__main__.py").write_text(source)
            bundle = work / "bundle"
            bundle.mkdir()
            (bundle / "__main__.py").write_text(source)
            with zipfile.ZipFile(work / "bundle.pyz", "w") as archive:
                archive.writestr("__main__.py", source)
            targets = [
                ("script", ["main.py"]),
                ("module", ["-m", "target_package.main"]),
                ("package", ["-m", "target_package"]),
                ("command", ["-c", source]),
                ("directory", ["bundle"]),
                ("zip", ["bundle.pyz"]),
            ]
            # GraalPy's launcher does not execute standalone pyc files. Its
            # product route uses that native launcher, not this wrapper.
            if sys.implementation.name != "graalpy":
                py_compile.compile(str(work / "main.py"), str(work / "entry.pyc"))
                targets.append(("bytecode", ["entry.pyc"]))
            for label, target in targets:
                with self.subTest(target=label):
                    arguments = [*target, "argument with spaces"]
                    direct = self.invoke(arguments, work, capture=False)
                    captured = self.invoke(arguments, work, capture=True)
                    self.assertEqual(direct.returncode, 7, direct.stderr)
                    self.assertEqual(captured.returncode, 7, captured.stderr)
                    self.assertEqual(
                        [json.loads(line) for line in captured.stdout.splitlines()],
                        [json.loads(line) for line in direct.stdout.splitlines()],
                    )
                    self.assertEqual(captured.stderr, direct.stderr)


if __name__ == "__main__":
    unittest.main()
