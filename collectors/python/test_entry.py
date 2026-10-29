import json
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest


HERE = Path(__file__).resolve().parent
RUN_TARGET = (
    "import sys; "
    f"sys.path.insert(0, {str(HERE)!r}); "
    "from hunin_capture.target import run_target; "
    "run_target(sys.argv[1:])"
)


class TargetEntryTest(unittest.TestCase):
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


if __name__ == "__main__":
    unittest.main()
