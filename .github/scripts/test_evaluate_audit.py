import copy
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import evaluate_audit


FIXTURE = Path(__file__).with_name('audit-inactive-rsa.fixture.json')
YANKED_FIXTURE = Path(__file__).with_name('audit-with-yanked.fixture.json')
DIGEST = 'a' * 64


def terminal_fixture(document, lock_path='/fixture/Cargo.lock', db_path='/fixture/advisory-db'):
    records = []
    for entry in document['vulnerabilities']['list']:
        records.append('Crate:    ' + entry['package']['name'] + '\nVersion:  ' + entry['package']['version'] + '\nID:       ' + entry['advisory']['id'] + '\n')
    for kind, entries in document['warnings'].items():
        for entry in entries:
            record = 'Crate:    ' + entry['package']['name'] + '\nVersion:  ' + entry['package']['version'] + '\nWarning:  ' + kind + '\n'
            if entry.get('advisory'):
                record += 'ID:       ' + entry['advisory']['id'] + '\n'
            records.append(record)
    stderr = [
        '    Fetching advisory database from `https://github.com/RustSec/advisory-db.git`',
        '      Loaded ' + str(document['database']['advisory-count']) + ' security advisories (from ' + db_path + ')',
        '    Updating crates.io index',
        '    Scanning ' + lock_path + ' for vulnerabilities (' + str(document['lockfile']['dependency-count']) + ' crate dependencies)',
    ]
    count = document['vulnerabilities']['count']
    if count:
        stderr.append('error: ' + str(count) + (' vulnerability' if count == 1 else ' vulnerabilities') + ' found!')
    denied = len(document['warnings'].get('yanked', []))
    allowed = sum(len(entries) for kind, entries in document['warnings'].items() if kind != 'yanked')
    if denied:
        stderr.append('error: ' + str(denied) + (' denied warning' if denied == 1 else ' denied warnings') + ' found!')
    if allowed:
        stderr.append('warning: ' + str(allowed) + (' allowed warning' if allowed == 1 else ' allowed warnings') + ' found')
    return '\n'.join(records), '\n'.join(stderr) + '\n'


class GuardedAudit(unittest.TestCase):
    def fixture(self):
        value = json.loads(FIXTURE.read_text())
        return value

    def decide(self, value=None, **changes):
        arguments = dict(document=self.fixture() if value is None else value, audit_exit=1, tree_exit=0, tree_stdout='', lock_before=DIGEST, lock_after=DIGEST)
        good = self.fixture()
        human_stdout, human_stderr = terminal_fixture(good)
        document = arguments['document']
        if isinstance(document.get('database'), dict) and isinstance(document.get('warnings'), dict) and all(isinstance(entries, list) for entries in document['warnings'].values()):
            human_stdout, human_stderr = terminal_fixture(document)
        arguments.update(audit_stderr='', human_exit=changes.get('audit_exit', 1), human_stdout=human_stdout, human_stderr=human_stderr, lock_path='/fixture/Cargo.lock', db_path='/fixture/advisory-db')
        arguments.update(changes)
        return evaluate_audit.evaluate(**arguments)

    def test_exact_inactive_finding_is_explicitly_waived_and_original_report_is_unchanged(self):
        original = self.fixture()
        before = copy.deepcopy(original)
        result = self.decide(original)
        self.assertEqual(result['status'], 'waived')
        self.assertEqual(result['original_audit_exit_code'], 1)
        self.assertEqual(result['waived'], [{'advisory': 'RUSTSEC-2023-0071', 'package': 'rsa', 'version': '0.9.10'}])
        self.assertEqual(original, before)
        self.assertEqual(result['warning_counts']['unsound'], 1)
        self.assertEqual(result['warning_counts']['unmaintained'], 8)

    def test_incomplete_yanked_check_is_not_treated_as_empty(self):
        stdout, stderr = terminal_fixture(self.fixture())
        for evidence in ['', stderr.replace('    Updating crates.io index\n', ''), stderr.replace('    Scanning', "warning: couldn't update crates.io index: fixture failure\n    Scanning"), stderr + "error: couldn't check if the package is yanked: fixture failure\n"]:
            with self.subTest(evidence=evidence):
                self.assertEqual(self.decide(human_stderr=evidence)['status'], 'failed')
        self.assertEqual(self.decide(audit_stderr="error: couldn't check if the package is yanked: fixture failure\n")['status'], 'failed')

    def test_terminal_evidence_requires_exact_paths_order_counts_and_complete_tail(self):
        stdout, stderr = terminal_fixture(self.fixture())
        lines = stderr.splitlines()
        candidates = [
            stderr.replace('/fixture/Cargo.lock', '/other/Cargo.lock'),
            stderr.replace('/fixture/advisory-db', '/other/advisory-db'),
            stderr.replace('(713 crate dependencies)', '(712 crate dependencies)'),
            stderr.replace('Loaded 1290', 'Loaded 1289'),
            '\n'.join(lines[:2] + [lines[3], lines[2]] + lines[4:]),
            stderr + lines[3] + '\n',
            stderr + lines[-1] + '\n',
            '\n'.join(lines[:-1]),
            stderr + 'warning: a future unknown diagnostic\n',
            stderr.replace('error: 1 vulnerability found!', 'error: 1 denied warning found!'),
        ]
        for evidence in candidates:
            with self.subTest(evidence=evidence):
                self.assertEqual(self.decide(human_stderr=evidence)['status'], 'failed')
        self.assertEqual(self.decide(human_exit=0)['status'], 'failed')
        self.assertEqual(self.decide(human_exit=101)['status'], 'failed')

    def test_same_counts_but_different_or_duplicate_terminal_findings_fail(self):
        stdout, stderr = terminal_fixture(self.fixture())
        candidates = ['', stdout.replace('RUSTSEC-2023-0071', 'RUSTSEC-2099-9999'), stdout.replace('Crate:    rsa', 'Crate:    other'), stdout.replace('Version:  0.9.10', 'Version:  0.9.11'), stdout + stdout, stdout.replace('Warning:  unsound', 'Warning:  notice'), stdout.replace('Version:  0.9.10', 'Version:  0.9.10\nVersion:  0.9.10')]
        for evidence in candidates:
            with self.subTest(evidence=evidence):
                self.assertEqual(self.decide(human_stdout=evidence)['status'], 'failed')

    def test_clean_original_pass_also_requires_yanked_completion(self):
        document = self.fixture()
        document['vulnerabilities'] = {'found': False, 'count': 0, 'list': []}
        for changes in [{'human_stderr': ''}, {'human_exit': 101}, {'audit_stderr': 'warning: index unavailable'}]:
            self.assertEqual(self.decide(document, audit_exit=0, **changes)['status'], 'failed')
        document['warnings'] = {}
        self.assertEqual(self.decide(document, audit_exit=0)['status'], 'passed')

    def test_real_terminal_output_matches_latest_actual_json(self):
        root = Path(__file__).parent
        stdout = (root / 'audit-terminal.stdout.fixture').read_text()
        stderr = (root / 'audit-terminal.stderr.fixture').read_text()
        result = self.decide(human_stdout=stdout, human_stderr=stderr)
        self.assertEqual(result['status'], 'waived')
        self.assertEqual(result['yanked_check']['status'], 'complete')
        self.assertEqual(self.decide(human_stdout=stdout, human_stderr=stderr.replace('    Updating crates.io index\n', ''))['status'], 'failed')

    def test_original_actual_report_still_fails_for_yanked(self):
        self.assertEqual(self.decide(json.loads(YANKED_FIXTURE.read_text()))['status'], 'failed')

    def test_active_graph_error_timeout_and_lock_change_fail(self):
        for changes in [{'tree_stdout': 'rsa v0.9.10\n'}, {'tree_stdout': 'unexpected output\n'}, {'tree_exit': 101}, {'tree_exit': None}, {'lock_after': 'b' * 64}, {'lock_before': 'not a hash'}, {'audit_exit': 101}, {'audit_exit': None}]:
            with self.subTest(changes=changes):
                self.assertEqual(self.decide(**changes)['status'], 'failed')

    def test_only_exact_id_package_and_version_can_be_waived(self):
        for field, value in [('id', 'RUSTSEC-2099-0001'), ('package', 'other')]:
            document = self.fixture()
            document['vulnerabilities']['list'][0]['advisory'][field] = value
            self.assertEqual(self.decide(document)['status'], 'failed')
        for field, value in [('name', 'other'), ('version', '0.9.11')]:
            document = self.fixture()
            document['vulnerabilities']['list'][0]['package'][field] = value
            self.assertEqual(self.decide(document)['status'], 'failed')

    def test_an_additional_vulnerability_fails(self):
        document = self.fixture()
        another = copy.deepcopy(document['vulnerabilities']['list'][0])
        another['advisory']['id'] = 'RUSTSEC-2099-0002'
        document['vulnerabilities']['list'].append(another)
        document['vulnerabilities']['count'] = 2
        self.assertEqual(self.decide(document)['status'], 'failed')

    def test_missing_or_inconsistent_schema_and_hidden_settings_fail(self):
        mutations = [
            lambda d: d.pop('database'),
            lambda d: d['database'].update({'advisory-count': 0}),
            lambda d: d['database'].update({'last-commit': 'bad'}),
            lambda d: d['settings'].update(ignore=['RUSTSEC-2099-0001']),
            lambda d: d['settings'].update(target_os=['macos']),
            lambda d: d['settings'].update(severity='high'),
            lambda d: d['warnings'].update(unexpected=[]),
            lambda d: d['warnings'].update(yanked='invalid'),
            lambda d: d['vulnerabilities'].update(count=0),
            lambda d: d['vulnerabilities'].update(found=False),
            lambda d: d.update(error='fetch failed'),
        ]
        for mutate in mutations:
            document = self.fixture()
            mutate(document)
            with self.subTest(mutation=mutate):
                self.assertEqual(self.decide(document)['status'], 'failed')

    def test_malformed_finding_entries_produce_failed_decisions(self):
        for entry in [None, 7, [], 'invalid', {}]:
            document = self.fixture()
            document['vulnerabilities']['list'] = [entry]
            with self.subTest(entry=entry):
                self.assertEqual(self.decide(document=document)['status'], 'failed')

    def test_clean_original_success_passes_without_a_waiver(self):
        document = self.fixture()
        document['vulnerabilities'] = {'found': False, 'count': 0, 'list': []}
        result = self.decide(document, audit_exit=0)
        self.assertEqual(result['status'], 'passed')
        self.assertEqual(result['waived'], [])
        self.assertEqual(self.decide(document, audit_exit=1)['status'], 'failed')
        self.assertEqual(self.decide(audit_exit=0)['status'], 'failed')

    def test_cli_keeps_raw_artifacts_and_writes_separate_decision(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            audit = root / 'audit.json'
            raw = json.dumps(self.fixture()).encode()
            audit.write_bytes(raw)
            lock = root / 'Cargo.lock'
            lock.write_bytes(b'fixture lock')
            before = root / 'lock-before.sha256'
            before.write_text(hashlib.sha256(lock.read_bytes()).hexdigest())
            tree = root / 'rsa-tree.stdout'
            tree.write_text('')
            human_stdout, human_stderr = terminal_fixture(self.fixture(), str(lock), str(root / 'advisory-db'))
            (root / 'human.stdout').write_text(human_stdout)
            (root / 'human.stderr').write_text(human_stderr)
            (root / 'audit.stderr').write_text('')
            decision = root / 'decision.json'
            args = ['--audit-json', str(audit), '--audit-exit', '1', '--tree-stdout', str(tree), '--tree-exit', '0', '--lock-before-file', str(before), '--lockfile', str(lock), '--decision', str(decision)]
            args.extend(['--audit-stderr', str(root / 'audit.stderr'), '--human-stdout', str(root / 'human.stdout'), '--human-stderr', str(root / 'human.stderr'), '--human-exit', '1', '--db-path', str(root / 'advisory-db')])
            self.assertEqual(evaluate_audit.main(args), 0)
            result = json.loads(decision.read_text())
            self.assertEqual(result['status'], 'waived')
            self.assertEqual(result['audit_json_sha256'], hashlib.sha256(raw).hexdigest())
            self.assertEqual(audit.read_bytes(), raw)
            malformed = root / 'malformed.json'
            malformed.write_text('{invalid')
            args[1] = str(malformed)
            args[args.index('--decision') + 1] = str(root / 'failed.json')
            self.assertEqual(evaluate_audit.main(args), 1)
            self.assertEqual(json.loads((root / 'failed.json').read_text())['status'], 'failed')


if __name__ == '__main__':
    unittest.main()
