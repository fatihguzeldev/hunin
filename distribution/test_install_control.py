import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("verify_install", Path(__file__).with_name("verify_install.py"))
control = importlib.util.module_from_spec(spec)
spec.loader.exec_module(control)


class InstalledControlTests(unittest.TestCase):
    def test_adjacent_import_compares_decoded_file_identity_and_directory_boundary(self):
        with tempfile.TemporaryDirectory() as temporary:
            installed = Path(temporary) / "installed bundle %20"
            preload = installed / "collectors/javascript/clock-preload.mjs"
            preload.parent.mkdir(parents=True)
            preload.write_text("// installed control")
            self.assertTrue(control.adjacent_import(f'--cpu-prof --import="{preload.as_uri()}"', installed))
            self.assertTrue(control.adjacent_import(f'--import "{preload.as_uri()}"', installed))
            outside = installed.with_name(installed.name + "-other") / "clock-preload.mjs"
            self.assertFalse(control.adjacent_import(f'--import="{outside.as_uri()}"', installed))
            self.assertFalse(control.adjacent_import('--import=data:text/javascript,0', installed))


if __name__ == "__main__":
    unittest.main()
