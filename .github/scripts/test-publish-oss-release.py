"""Offline regression checks for canonical and legacy release promotion."""

import importlib.util
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch, Mock


spec = importlib.util.spec_from_file_location(
    "publisher", Path(__file__).with_name("publish-oss-release.py")
)
publisher = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publisher)

CANONICAL = "https://new.example/releases"
LEGACY = "https://old.example/releases"
TAG = "v1.2.3"


class PublisherTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.source = Path(directory.name)
        for name in ("app.tar.gz", "setup.exe", "install.sh"):
            (self.source / name).write_bytes(name.encode())
        self.manifest = {
            "version": "1.2.3",
            "platforms": {
                platform: {
                    "signature": "signed-" + platform,
                    "url": "https://github.example/" + (
                        "setup.exe" if platform.startswith("windows") else "app.tar.gz"
                    ),
                }
                for platform in publisher.PLATFORMS
            },
        }
        self.save_manifest()
        self.bucket = Mock()
        self.bucket.head_object.side_effect = lambda key: SimpleNamespace(
            content_length=(self.source / Path(key).name).stat().st_size
        )
        self.enterContext(patch.dict(os.environ, {
            "ALIYUN_ACCESS_KEY_ID": "fixture", "ALIYUN_ACCESS_KEY_SECRET": "fixture",
        }, clear=True))
        self.enterContext(patch.object(publisher.oss2, "Bucket", return_value=self.bucket))
        self.upload = self.enterContext(patch.object(publisher, "upload"))
        self.verify_manifest = self.enterContext(patch.object(publisher, "verify_public_manifest"))
        self.enterContext(patch.object(publisher, "verify_public_object"))

    def save_manifest(self):
        (self.source / "latest.json").write_text(json.dumps(self.manifest))

    def run_phase(self, phase="mirror-latest"):
        argv = [
            "publisher", phase, "--source-dir", str(self.source),
            "--bucket", "fixture", "--endpoint", "https://oss.example",
            "--tag", TAG, "--public-base-url", LEGACY if phase == "mirror-latest" else CANONICAL,
        ]
        if phase == "mirror-latest":
            argv += ["--asset-base-url", CANONICAL]
        with patch.object(publisher.sys, "argv", argv):
            publisher.main()

    def test_mirror_only_updates_latest_and_preserves_signed_asset_references(self):
        self.run_phase()
        self.bucket.head_object.assert_not_called()
        calls = self.upload.call_args_list
        self.assertEqual(len(calls), 4)
        self.assertTrue(all(call.args[1].startswith("releases/latest/") for call in calls))
        self.assertEqual(calls[-1].args[1], "releases/latest/latest.json")
        result = json.loads(calls[-1].args[3])
        for platform, entry in result["platforms"].items():
            self.assertEqual(entry["signature"], self.manifest["platforms"][platform]["signature"])
            self.assertTrue(entry["url"].startswith(CANONICAL + "/" + TAG + "/"))
        self.assertEqual(self.verify_manifest.call_args_list[0].args, (
            CANONICAL + "/latest/latest.json", TAG, CANONICAL,
        ))
        self.assertEqual(self.verify_manifest.call_args_list[-1].args, (
            LEGACY + "/latest/latest.json", TAG, CANONICAL,
        ))

    def test_mismatched_source_version_never_writes(self):
        self.manifest["version"] = "1.2.2"
        self.save_manifest()
        with self.assertRaisesRegex(SystemExit, "version"):
            self.run_phase()
        self.upload.assert_not_called()

    def test_missing_signature_never_writes(self):
        self.manifest["platforms"]["windows-x86_64"]["signature"] = ""
        self.save_manifest()
        with self.assertRaisesRegex(SystemExit, "signature"):
            self.run_phase()
        self.upload.assert_not_called()

    def test_unverified_canonical_release_never_writes(self):
        self.verify_manifest.side_effect = SystemExit("canonical release unavailable")
        with self.assertRaises(SystemExit):
            self.run_phase()
        self.upload.assert_not_called()

    def test_newer_canonical_release_blocks_stale_manifest_publication(self):
        self.verify_manifest.side_effect = [None, SystemExit("canonical version changed")]
        with self.assertRaises(SystemExit):
            self.run_phase()
        self.assertEqual(self.upload.call_count, 3)
        self.assertFalse(any(call.args[2] == "latest.json" for call in self.upload.call_args_list))

    def test_primary_promotion_requires_immutable_assets(self):
        self.run_phase("promote")
        self.assertEqual(self.bucket.head_object.call_count, 3)
        self.assertTrue(all(
            call.args[0].startswith("releases/" + TAG + "/")
            for call in self.bucket.head_object.call_args_list
        ))
        self.assertEqual(self.upload.call_args_list[-1].args[2], "latest.json")


if __name__ == "__main__":
    unittest.main()
