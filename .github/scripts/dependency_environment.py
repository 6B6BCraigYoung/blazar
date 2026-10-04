import os
import sys


def isolated_git_environment(environment):
    omitted = {
        'GIT_CONFIG', 'GIT_ASKPASS', 'SSH_ASKPASS', 'SSH_ASKPASS_REQUIRE',
        'GH_TOKEN', 'GITHUB_TOKEN', 'GH_ENTERPRISE_TOKEN', 'GITHUB_ENTERPRISE_TOKEN',
        'GIT_DIR', 'GIT_WORK_TREE', 'GIT_COMMON_DIR', 'GIT_INDEX_FILE',
        'GIT_OBJECT_DIRECTORY', 'GIT_ALTERNATE_OBJECT_DIRECTORIES',
        'GIT_CEILING_DIRECTORIES', 'GIT_DISCOVERY_ACROSS_FILESYSTEM',
        'GIT_CURL_VERBOSE', 'GIT_SSH', 'GIT_SSH_COMMAND',
    }
    env = {name: value for name, value in environment.items() if name not in omitted and not name.startswith(('GIT_CONFIG_', 'GIT_TRACE'))}
    env.update(
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_CONFIG_SYSTEM=os.devnull,
        GIT_CONFIG_NOSYSTEM='1',
        GIT_TERMINAL_PROMPT='0',
        GIT_CONFIG_COUNT='1',
        GIT_CONFIG_KEY_0='credential.helper',
        GIT_CONFIG_VALUE_0='',
    )
    return env


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv or argv.pop(0) != '--' or not argv:
        raise ValueError('Usage: dependency_environment.py -- command [arguments]')
    os.execvpe(argv[0], argv, isolated_git_environment(os.environ))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError) as error:
        raise SystemExit(str(error))
