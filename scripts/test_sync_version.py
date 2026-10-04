import contextlib
import io
import json
import stat
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import sync_version


class VersionFixtures(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='blazar-version-fixture-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.config = self.root / 'apps/desktop/tauri.conf.json'
        self.config.parent.mkdir(parents=True)
        self.config.write_text('{"version":"0.1.0","identifier":"ai.example.app","extra":{"name":"示例","values":[1,true,null]}}\n')
        self.cargo = self.root / 'Cargo.toml'
        self.cargo.write_text('[workspace.package]\nversion="1.2.3"\n')
        (self.root / 'Cargo.lock').write_text('fixture lock is never changed\n')

    def call(self, *args):
        stdout, stderr = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            code = sync_version.main(['--root', str(self.root), *args])
        return code, stdout.getvalue(), stderr.getvalue()

    def test_check_detects_drift_without_writing(self):
        before = self.config.read_bytes()
        timestamp = self.config.stat().st_mtime_ns
        self.assertEqual(self.call('--check')[0], 1)
        self.assertEqual(self.config.read_bytes(), before)
        self.assertEqual(self.config.stat().st_mtime_ns, timestamp)

    def test_write_changes_only_version_semantics_and_preserves_mode(self):
        before = json.loads(self.config.read_text())
        self.config.chmod(0o640)
        cargo = self.cargo.read_bytes()
        lock = (self.root / 'Cargo.lock').read_bytes()
        self.assertEqual(self.call('--write')[0], 0)
        after = json.loads(self.config.read_text())
        self.assertEqual(after.pop('version'), '1.2.3')
        before.pop('version')
        self.assertEqual(after, before)
        self.assertEqual(stat.S_IMODE(self.config.stat().st_mode), 0o640)
        self.assertEqual(self.cargo.read_bytes(), cargo)
        self.assertEqual((self.root / 'Cargo.lock').read_bytes(), lock)

    def test_second_write_is_byte_and_timestamp_noop(self):
        self.assertEqual(self.call('--write')[0], 0)
        before = self.config.read_bytes()
        timestamp = self.config.stat().st_mtime_ns
        self.assertEqual(self.call('--write')[0], 0)
        self.assertEqual(self.config.read_bytes(), before)
        self.assertEqual(self.config.stat().st_mtime_ns, timestamp)
        self.assertEqual(self.call('--check')[0], 0)

    def test_print_and_valid_prerelease(self):
        self.cargo.write_text('[workspace.package]\nversion="1.2.3-rc.1+build.7"\n')
        before = self.config.read_bytes()
        self.assertEqual(self.call('--print'), (0, '1.2.3-rc.1+build.7\n', ''))
        self.assertEqual(self.config.read_bytes(), before)
        self.assertEqual(self.call('--write')[0], 0)
        self.assertEqual(self.call('--check', '--tag', 'v1.2.3-rc.1+build.7')[0], 0)

    def test_tag_mismatch_is_nonwriting(self):
        before = self.config.read_bytes()
        self.assertEqual(self.call('--check', '--tag', 'v9.9.9')[0], 1)
        self.assertEqual(self.config.read_bytes(), before)

    def test_invalid_and_missing_cargo_versions_are_rejected(self):
        before = self.config.read_bytes()
        for content in ['[workspace]\n', '[workspace.package]\nversion=123\n', '[workspace.package]\nversion="01.2.3"\n', '[workspace.package]\nversion="1.2.3-01"\n', '[workspace.package]\nversion="1.2"\n', '[workspace.package]\nversion=""\n', 'not valid toml']:
            with self.subTest(content=content):
                self.cargo.write_text(content)
                self.assertEqual(self.call('--write')[0], 2)
                self.assertEqual(self.config.read_bytes(), before)

    def test_bad_json_top_level_and_duplicate_keys_are_rejected(self):
        for content in ['{invalid', '[]', '{"version":"0.1.0","version":"0.2.0"}']:
            with self.subTest(content=content):
                self.config.write_text(content)
                self.assertEqual(self.call('--write')[0], 2)
                self.assertEqual(self.config.read_text(), content)

    def test_failed_replace_keeps_original_and_removes_temp(self):
        before = self.config.read_bytes()
        with patch.object(sync_version.os, 'replace', side_effect=OSError('fixture replacement failed')):
            self.assertEqual(self.call('--write')[0], 2)
        self.assertEqual(self.config.read_bytes(), before)
        self.assertEqual(list(self.config.parent.iterdir()), [self.config])

    def test_synchronized_version_does_not_hide_nonstandard_json_numbers(self):
        for invalid in ['NaN', 'Infinity', '-Infinity']:
            with self.subTest(invalid=invalid):
                content = '{"version":"1.2.3","invalid":' + invalid + '}'
                self.config.write_text(content)
                self.assertEqual(self.call('--check')[0], 2)
                self.assertEqual(self.call('--write')[0], 2)
                self.assertEqual(self.config.read_text(), content)

    def test_symlink_destination_is_rejected(self):
        alternate = self.root / 'alternate.json'
        alternate.write_text('{"version":"0.0.1"}')
        self.config.unlink()
        self.config.symlink_to(alternate)
        self.assertEqual(self.call('--write')[0], 2)
        self.assertEqual(alternate.read_text(), '{"version":"0.0.1"}')


if __name__ == '__main__':
    unittest.main()
