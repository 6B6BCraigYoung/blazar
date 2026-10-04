import argparse
import hashlib
import json
import re
from collections import Counter
from pathlib import Path


EXPECTED = {'advisory': 'RUSTSEC-2023-0071', 'package': 'rsa', 'version': '0.9.10'}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def finding_identity(entry, kind):
    require(isinstance(entry, dict), 'Malformed finding entry')
    package = entry.get('package')
    advisory = entry.get('advisory')
    require(isinstance(package, dict) and isinstance(package.get('name'), str) and isinstance(package.get('version'), str), 'Malformed finding package')
    require(advisory is None or isinstance(advisory, dict) and isinstance(advisory.get('id'), str), 'Malformed finding advisory')
    return package['name'], package['version'], kind, advisory['id'] if advisory else None


def terminal_findings(stdout):
    identities = []
    current = None
    for line in stdout.splitlines():
        match = re.fullmatch(r'(Crate|Version|Warning|ID):\s*(.+)', line.strip())
        if not match:
            continue
        field, value = match.groups()
        if field == 'Crate':
            if current is not None:
                require('Version' in current, 'Incomplete terminal finding')
                identities.append((current['Crate'], current['Version'], current.get('Warning'), current.get('ID')))
            current = {'Crate': value}
        else:
            require(current is not None and field not in current, 'Unexpected or duplicated terminal finding field')
            current[field] = value
    if current is not None:
        require('Version' in current, 'Incomplete terminal finding')
        identities.append((current['Crate'], current['Version'], current.get('Warning'), current.get('ID')))
    return Counter(identities)


def require_complete_terminal(document, audit_exit, audit_stderr, human_exit, human_stdout, human_stderr, lock_path, db_path):
    require(isinstance(audit_stderr, str) and not audit_stderr.strip(), 'JSON audit emitted diagnostics; completion is unproven')
    require(type(human_exit) is int and human_exit == audit_exit, 'Terminal audit did not complete with the same result')
    require(isinstance(human_stdout, str) and isinstance(human_stderr, str), 'Missing terminal audit evidence')
    require(isinstance(lock_path, str) and isinstance(db_path, str) and Path(lock_path).is_absolute() and Path(db_path).is_absolute(), 'Missing exact audit paths')
    patterns = [
        r'Fetching\s+advisory database from `https://github\.com/RustSec/advisory-db\.git`',
        r'Loaded\s+' + str(document['database']['advisory-count']) + r' security advisories \(from ' + re.escape(db_path) + r'\)',
        r'Updating\s+crates\.io index',
        r'Scanning\s+' + re.escape(lock_path) + r' for vulnerabilities \(' + str(document['lockfile']['dependency-count']) + r' crate dependencies\)',
    ]
    count = document['vulnerabilities']['count']
    if count:
        patterns.append('error: ' + str(count) + (' vulnerability' if count == 1 else ' vulnerabilities') + ' found!')
    allowed = sum(len(entries) for entries in document['warnings'].values())
    if allowed:
        patterns.append('warning: ' + str(allowed) + (' allowed warning' if allowed == 1 else ' allowed warnings') + ' found')
    lines = [line.strip() for line in human_stderr.splitlines() if line.strip()]
    require(len(lines) == len(patterns) and all(re.fullmatch(pattern, line) for pattern, line in zip(patterns, lines)), 'Terminal audit completion chain is missing, reordered, or contains unexpected diagnostics')
    expected = [finding_identity(entry, None) for entry in document['vulnerabilities']['list']]
    for kind, entries in document['warnings'].items():
        expected.extend(finding_identity(entry, kind) for entry in entries)
    require(terminal_findings(human_stdout) == Counter(expected), 'JSON and terminal finding identities differ')


def evaluate(document, audit_exit, tree_exit, tree_stdout, lock_before, lock_after, audit_stderr=None, human_exit=None, human_stdout=None, human_stderr=None, lock_path=None, db_path=None):
    result = {'status': 'failed', 'original_audit_exit_code': audit_exit, 'tree_exit_code': tree_exit, 'waived': []}
    try:
        require(isinstance(lock_before, str) and re.fullmatch('[0-9a-f]{64}', lock_before), 'Missing or invalid initial lock digest')
        require(lock_before == lock_after, 'Cargo.lock changed during audit and graph checks')
        require(type(audit_exit) is int and audit_exit in (0, 1), 'Audit did not complete with an expected result code')
        require(isinstance(document, dict) and set(document) == {'database', 'lockfile', 'settings', 'vulnerabilities', 'warnings'}, 'Unexpected audit report structure')
        database = document['database']
        require(isinstance(database, dict), 'Missing advisory database evidence')
        require(type(database.get('advisory-count')) is int and database['advisory-count'] > 0, 'Empty advisory database')
        require(isinstance(database.get('last-commit'), str) and re.fullmatch('[0-9a-f]{40}', database['last-commit']), 'Invalid advisory database commit')
        require(isinstance(database.get('last-updated'), str) and bool(database['last-updated']), 'Missing advisory database timestamp')
        lock = document['lockfile']
        require(isinstance(lock, dict) and type(lock.get('dependency-count')) is int and lock['dependency-count'] > 0, 'Missing lockfile scan evidence')
        settings = document['settings']
        expected_settings = {'target_arch': [], 'target_os': [], 'severity': None, 'ignore': [], 'informational_warnings': ['unmaintained', 'unsound', 'notice']}
        require(settings == expected_settings, 'Audit filters or settings differ from the unfiltered policy')
        warnings = document['warnings']
        require(isinstance(warnings, dict) and set(warnings) <= {'unmaintained', 'unsound', 'notice', 'yanked'}, 'Unexpected warning category')
        require(all(isinstance(entries, list) and all(isinstance(entry, dict) for entry in entries) for entries in warnings.values()), 'Malformed warning list')
        result['warning_counts'] = {kind: len(entries) for kind, entries in warnings.items()}
        require(not warnings.get('yanked'), 'Yanked dependencies remain denied')
        vulnerabilities = document['vulnerabilities']
        require(isinstance(vulnerabilities, dict) and set(vulnerabilities) == {'found', 'count', 'list'}, 'Malformed vulnerability summary')
        entries = vulnerabilities['list']
        require(isinstance(entries, list) and type(vulnerabilities['count']) is int and vulnerabilities['count'] == len(entries), 'Inconsistent vulnerability count')
        require(type(vulnerabilities['found']) is bool and vulnerabilities['found'] == bool(entries), 'Inconsistent vulnerability status')
        require_complete_terminal(document, audit_exit, audit_stderr, human_exit, human_stdout, human_stderr, lock_path, db_path)
        result['yanked_check'] = {'status': 'complete', 'method': 'cargo-audit-0.22.2-terminal-control-flow', 'dependency_count': lock['dependency-count']}
        if not entries:
            require(audit_exit == 0, 'Audit failed without an explained vulnerability or yanked result')
            result.update(status='passed', reason='Unfiltered audit passed without a waiver')
            return result
        require(audit_exit == 1, 'Audit result code is inconsistent with reported vulnerabilities')
        require(len(entries) == 1 and isinstance(entries[0], dict), 'Additional vulnerabilities cannot be waived')
        entry = entries[0]
        advisory, package = entry.get('advisory'), entry.get('package')
        require(isinstance(advisory, dict) and isinstance(package, dict), 'Malformed vulnerability entry')
        actual = {'advisory': advisory.get('id'), 'package': package.get('name'), 'version': package.get('version')}
        require(actual == EXPECTED and advisory.get('package') == 'rsa', 'The finding is outside the exact approved advisory and package version')
        require(advisory.get('withdrawn') is None and advisory.get('informational') is None, 'Unexpected advisory classification')
        require(type(tree_exit) is int and tree_exit == 0, 'The all-target active dependency graph check failed')
        require(isinstance(tree_stdout, str) and not tree_stdout.strip(), 'RSA appears in the active graph or the graph output is unexpected')
        result.update(status='waived', waived=[dict(EXPECTED)], reason='Exact RSA lockfile finding waived only because the full active graph is empty; original audit failed')
    except ValueError as error:
        result['reason'] = str(error)
    return result


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'Duplicate JSON key')
        result[key] = value
    return result


def reject_constant(value):
    raise ValueError('Nonfinite JSON value')


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument('--audit-json', type=Path, required=True)
    parser.add_argument('--audit-exit', type=int, required=True)
    parser.add_argument('--audit-stderr', type=Path, required=True)
    parser.add_argument('--human-stdout', type=Path, required=True)
    parser.add_argument('--human-stderr', type=Path, required=True)
    parser.add_argument('--human-exit', type=int, required=True)
    parser.add_argument('--db-path', type=Path, required=True)
    parser.add_argument('--tree-stdout', type=Path, required=True)
    parser.add_argument('--tree-exit', type=int, required=True)
    parser.add_argument('--lock-before-file', type=Path, required=True)
    parser.add_argument('--lockfile', type=Path, required=True)
    parser.add_argument('--decision', type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        raw = args.audit_json.read_bytes()
        document = json.loads(raw, object_pairs_hook=unique_object, parse_constant=reject_constant)
        tree_raw = args.tree_stdout.read_bytes()
        lock_before = args.lock_before_file.read_text().strip()
        lock_after = hashlib.sha256(args.lockfile.read_bytes()).hexdigest()
        human_stdout = args.human_stdout.read_bytes()
        human_stderr = args.human_stderr.read_bytes()
        audit_stderr = args.audit_stderr.read_bytes()
        result = evaluate(document, args.audit_exit, args.tree_exit, tree_raw.decode(), lock_before, lock_after, audit_stderr.decode(), args.human_exit, human_stdout.decode(), human_stderr.decode(), str(args.lockfile), str(args.db_path))
        result.update(human_stdout_sha256=hashlib.sha256(human_stdout).hexdigest(), human_stderr_sha256=hashlib.sha256(human_stderr).hexdigest(), audit_stderr_sha256=hashlib.sha256(audit_stderr).hexdigest(), human_audit_exit_code=args.human_exit)
        result.update(audit_json_sha256=hashlib.sha256(raw).hexdigest(), tree_stdout_sha256=hashlib.sha256(tree_raw).hexdigest(), lock_sha256_before=lock_before, lock_sha256_after=lock_after)
    except (OSError, ValueError) as error:
        result = {'status': 'failed', 'original_audit_exit_code': args.audit_exit, 'tree_exit_code': args.tree_exit, 'waived': [], 'reason': 'Could not validate complete audit inputs: ' + type(error).__name__}
    with args.decision.open('x') as output:
        json.dump(result, output, indent=2)
        output.write('\n')
    if result['status'] == 'waived':
        print('WARNING: RUSTSEC-2023-0071 / rsa 0.9.10 remains in Cargo.lock; this run explicitly waives it only after an empty all-target active graph. Original cargo-audit exited 1.')
    elif result['status'] == 'failed':
        print('Audit policy failed: ' + result['reason'])
    return 0 if result['status'] in ('passed', 'waived') else 1


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except OSError as error:
        raise SystemExit('Unable to retain audit decision: ' + type(error).__name__)
