import importlib.util
import json
from pathlib import Path
import plistlib
import tempfile
import unittest
from unittest.mock import patch
import subprocess

spec = importlib.util.spec_from_file_location(
    "release_macos", Path(__file__).resolve().parents[1] / "release-macos.py"
)
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_only_stable_version_tags_are_accepted(self):
        self.assertEqual(release.release_tag("0.1.2"), "v0.1.2")
        for invalid in ["v0.1.2", "0.1", "0.1.2-beta.1", "0.1.2\n"]:
            with self.assertRaises(ValueError):
                release.release_tag(invalid)

    def test_retry_refuses_published_or_different_commit(self):
        for draft, target in [(False, "abc"), (True, "other")]:
            with self.assertRaises(ValueError):
                release.validate_existing({"isDraft": draft, "targetCommitish": target}, "abc")
        release.validate_existing({"isDraft": True, "targetCommitish": "abc"}, "abc")

    def test_failed_checks_do_not_build_or_upload(self):
        with tempfile.TemporaryDirectory() as directory:
            with patch.object(release, "ROOT", Path(directory)), \
                 patch.object(release.sys, "argv", ["release-macos.py", "--notes", "notes.md"]), \
                 patch.object(release, "preflight", return_value=("0.1.2", "v0.1.2", "abc")), \
                 patch.object(release, "run", side_effect=subprocess.CalledProcessError(1, "cargo fmt")) as run, \
                 patch.object(release, "upload_draft") as upload:
                with self.assertRaises(subprocess.CalledProcessError):
                    release.main()
                self.assertEqual(run.call_count, 1)
                upload.assert_not_called()

    def test_check_mode_does_not_build_or_upload(self):
        with patch.object(release.sys, "argv", ["release-macos.py", "--notes", "notes.md", "--check"]), \
             patch.object(release, "preflight", return_value=("0.1.2", "v0.1.2", "abc")), \
             patch.object(release, "run") as run, \
             patch.object(release, "upload_draft") as upload:
            release.main()
            run.assert_not_called()
            upload.assert_not_called()

    def test_bundle_requires_both_versions_and_expected_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory)
            (bundle / "Contents").mkdir()
            info = {"CFBundleIdentifier": release.BUNDLE_ID,
                    "CFBundleVersion": "0.1.2", "CFBundleShortVersionString": "0.1.2"}
            path = bundle / "Contents/Info.plist"
            path.write_bytes(plistlib.dumps(info))
            release.validate_bundle(bundle, "0.1.2")
            for field in info:
                changed = dict(info, **{field: "wrong"})
                path.write_bytes(plistlib.dumps(changed))
                with self.assertRaises(ValueError):
                    release.validate_bundle(bundle, "0.1.2")

    def test_upload_failure_leaves_a_draft_and_never_publishes(self):
        calls = []
        def run(*args, **kwargs):
            calls.append(args)
            if args[:3] == ("gh", "release", "upload"):
                raise subprocess.CalledProcessError(1, args)
        with patch.object(release, "find_release", side_effect=[None, {
            "isDraft": True, "targetCommitish": "abc"
        }]), patch.object(release, "run", side_effect=run):
            with self.assertRaises(subprocess.CalledProcessError):
                release.upload_draft("v0.1.2", "abc", Path("notes.md"), [])
        self.assertIn("--draft", calls[0])
        self.assertFalse(any("--draft=false" in args for args in calls))

    def test_draft_retry_verifies_uploaded_digest(self):
        draft = {"isDraft": True, "targetCommitish": "abc"}
        with tempfile.TemporaryDirectory() as directory:
            artifact = Path(directory) / "archive.zip"
            artifact.write_bytes(b"release")
            response = dict(draft, url="https://example.test/draft", assets=[{
                "name": artifact.name, "size": 7, "state": "uploaded", "digest": "wrong"
            }])
            with patch.object(release, "find_release", return_value=draft), \
                 patch.object(release, "run", return_value=json.dumps(response)) as run:
                with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                    release.upload_draft("v0.1.2", "abc", Path("notes.md"), [artifact])
                self.assertFalse(any(call.args[:3] == ("gh", "release", "create")
                                     for call in run.call_args_list))

    def test_changed_release_is_not_overwritten(self):
        with patch.object(release, "find_release", return_value={
            "isDraft": False, "targetCommitish": "abc"
        }), patch.object(release, "run") as run:
            with self.assertRaises(ValueError):
                release.upload_draft("v0.1.2", "abc", Path("notes.md"), [])
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
