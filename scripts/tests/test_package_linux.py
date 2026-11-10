import hashlib
import importlib.util
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("package_linux", ROOT / "scripts/package-linux.py")
package_linux = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package_linux)


class LinuxPackageTests(unittest.TestCase):
    def test_staged_launcher_matches_executable_and_icon(self):
        with tempfile.TemporaryDirectory() as directory:
            stage = Path(directory)
            package_linux.stage(Path(shutil.which("true")), stage)
            desktop = (stage / f"share/applications/{package_linux.APP_ID}.desktop").read_text()
            self.assertIn("Exec=ganymede\n", desktop)
            self.assertIn(f"Icon={package_linux.APP_ID}\n", desktop)
            self.assertEqual((stage / "bin/ganymede").stat().st_mode & 0o777, 0o755)
            self.assertTrue((stage / f"share/icons/hicolor/1024x1024/apps/{package_linux.APP_ID}.png").is_file())
            self.assertNotIn("MimeType=", desktop)  # CLI file opening is not implemented.

    @unittest.skipUnless(platform.system() == "Linux" and all(shutil.which(tool) for tool in
                         ("dpkg-shlibdeps", "dpkg-deb", "desktop-file-validate")), "needs Linux packaging tools")
    def test_packages_have_correct_paths_dependencies_and_checksums(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            arch = subprocess.check_output(["dpkg", "--print-architecture"], text=True).strip()
            archive, deb, manifest = package_linux.package(Path(shutil.which("true")), "1.2.3", arch, output)
            with tarfile.open(archive) as tar:
                names = tar.getnames()
                self.assertTrue(any(name.endswith("/bin/ganymede") for name in names))
                self.assertFalse(any("DEBIAN" in name for name in names))
                self.assertTrue(all(item.uid == 0 and item.gid == 0 for item in tar.getmembers()))
            metadata = subprocess.check_output(["dpkg-deb", "--field", str(deb)], text=True)
            self.assertIn("Version: 1.2.3", metadata)
            self.assertIn(f"Architecture: {arch}", metadata)
            self.assertIn("libc6", metadata)
            self.assertIn("xdg-desktop-portal", metadata)
            extracted = output / "extracted"
            subprocess.run(["dpkg-deb", "--extract", str(deb), str(extracted)], check=True)
            self.assertEqual((extracted / "usr/bin/ganymede").read_bytes(), Path(shutil.which("true")).read_bytes())
            for line in manifest.read_text().splitlines():
                digest, name = line.split()
                self.assertEqual(digest, hashlib.sha256((output / name).read_bytes()).hexdigest())
