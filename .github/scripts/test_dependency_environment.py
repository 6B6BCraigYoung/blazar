import os
import unittest
from unittest.mock import patch

import dependency_environment


class DependencyEnvironment(unittest.TestCase):
    def test_source_is_not_modified_and_inherited_configuration_is_replaced(self):
        original = {'GIT_CONFIG_COUNT': '999', 'GIT_CONFIG_KEY_998': 'include.path', 'GIT_CONFIG_VALUE_998': '/fixture/config', 'GH_TOKEN': 'fixture-secret', 'PATH': '/fixture/bin'}
        before = dict(original)
        env = dependency_environment.isolated_git_environment(original)
        self.assertEqual(original, before)
        self.assertNotIn('GIT_CONFIG_KEY_998', env)
        self.assertNotIn('GIT_CONFIG_VALUE_998', env)
        self.assertNotIn('GH_TOKEN', env)
        self.assertEqual(env['GIT_CONFIG_COUNT'], '1')
        self.assertEqual(env['GIT_CONFIG_KEY_0'], 'credential.helper')
        self.assertEqual(env['GIT_CONFIG_VALUE_0'], '')
        self.assertEqual(env['PATH'], '/fixture/bin')

    def test_wrapper_preserves_argument_boundaries_and_passes_isolated_environment(self):
        original = {'GITHUB_TOKEN': 'fixture-secret', 'HOME': '/fixture/home', 'CODEX_HOME': '/fixture/codex', 'HTTPS_PROXY': 'http://proxy.invalid:8080', 'GIT_SSL_CAINFO': '/fixture/ca.pem'}
        with patch.dict(os.environ, original, clear=True), patch.object(os, 'execvpe') as execute:
            dependency_environment.main(['--', '/fixture/tool with spaces', '--file', '/fixture/input with spaces'])
        executable, argv, env = execute.call_args.args
        self.assertEqual(executable, '/fixture/tool with spaces')
        self.assertEqual(argv, ['/fixture/tool with spaces', '--file', '/fixture/input with spaces'])
        self.assertNotIn('GITHUB_TOKEN', env)
        for name in ['HOME', 'CODEX_HOME', 'HTTPS_PROXY', 'GIT_SSL_CAINFO']:
            self.assertEqual(env[name], original[name])
        self.assertEqual(env['GIT_CONFIG_GLOBAL'], os.devnull)
        self.assertEqual(env['GIT_CONFIG_SYSTEM'], os.devnull)
        self.assertEqual(env['GIT_CONFIG_NOSYSTEM'], '1')
        self.assertEqual(env['GIT_TERMINAL_PROMPT'], '0')

    def test_missing_separator_or_command_does_not_execute(self):
        for argv in [[], ['--'], ['cargo', '--version']]:
            with self.subTest(argv=argv), patch.object(os, 'execvpe') as execute:
                with self.assertRaises(ValueError):
                    dependency_environment.main(argv)
                execute.assert_not_called()


if __name__ == '__main__':
    unittest.main()
