"""Read exact Cargo integration-test targets from shell steps in CI YAML.

Only explicit backslash-newline continuations are folded. Independent shell
commands are never joined, so batching must not mask dropped or moved targets.
"""
import re


def cargo_test_commands(source):
    """Return (package, targets, command) for each actual Cargo test command."""
    logical = re.sub(r"\\\r?\n[ \t]*", " ", source)
    commands = []
    for line in logical.splitlines():
        if line.lstrip().startswith("#"):
            continue
        match = re.search(r"\bcargo test -p ([A-Za-z0-9_-]+)(?=\s|$)", line)
        if match is None:
            continue
        command = line[match.start():].strip()
        # Test selectors occur before libtest's "--" argument separator.
        args = command.partition(" -- ")[0].split()
        targets = tuple(args[index + 1] for index, arg in enumerate(args[:-1])
                        if arg == "--test")
        commands.append((match.group(1), targets, command))
    return commands


def matching_cargo_test_commands(source, package, test_target):
    """Match the package and target inside the same logical shell command."""
    return [command for actual_package, targets, command in cargo_test_commands(source)
            if actual_package == package and test_target in targets]
