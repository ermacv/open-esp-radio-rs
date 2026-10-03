#!/usr/bin/env python3
"""PreToolUse guard for Claude Code sessions in this repository.

Reads the hook input JSON on stdin and blocks (exit 2, one line on stderr):

- a Read of a large generated file (raw PAC, PAC API catalog, published SVD
  or binding index, vendor fact registries): grep it with an explicit path;
- a Bash command that reads such a file whole (cat, head, sed, ...);
- a foreground Bash build, test, check or HIL run: rerun it with
  run_in_background;
- `pkill -f` / `pgrep -f` in any mode: wait on a PID or a job instead;
- `uhubctl` with an action (`-a`, `--action`): a hub port is switched only by
  `cargo hil board reset BOARD --via power`, a power cycle under the board's
  lease; reading the hub's state stays allowed.

Everything else exits 0 without output, leaving the normal permission flow.
Only the Python standard library is used.
"""

import json
import os
import re
import shlex
import sys

GENERATED = re.compile(
    r"(?:^|/)(?:"
    r"crates/hardware/[^/]+/pac/raw/src/lib\.rs"
    r"|crates/hardware/[^/]+/pac/src/generated\.rs"
    r"|registers/[^/]+/published/[^/]+\.(?:svd|bindings\.toml)"
    r"|verification/[^/]+/facts/provenance\.toml"
    r"|verification/[^/]+/facts/names/[^/]+\.toml"
    r")$"
)
GENERATED_HINT = (
    "is a large generated file: grep it with an explicit path "
    "(Grep with path=<file> and -C context) instead of reading it"
)

# Commands that print a file's contents when it is named as an argument.
READERS = {
    "cat", "tac", "less", "more", "bat", "batcat", "head", "tail", "sed",
    "awk", "gawk", "nl", "xxd", "od", "hexdump", "strings", "view",
}

# Wrappers that run their argument as the actual command.
WRAPPERS = {"command", "builtin", "noglob", "nohup", "exec", "time", "nice",
            "ionice", "stdbuf", "timeout", "env", "chrt", "taskset", "unbuffer"}
WRAPPER_VALUE_FLAGS = {"-n", "-c", "-k", "-s", "--signal", "--kill-after",
                       "-i", "-o", "-e", "-u", "--unset", "-C", "--chdir"}

CARGO_HEAVY = {"build", "b", "test", "t", "clippy", "check", "c", "doc", "d",
               "nextest", "bench", "miri"}
CARGO_VALUE_FLAGS = {"--color", "-C", "--config", "-Z", "--explain"}
XTASK_HEAVY = ("check", "push", "build", "doc", "compare", "evidence",
               "vendor-scenario", "vendor-firmware")
HIL_HEAVY = {"run", "run-all", "wait", "lease", "flash"}

BACKGROUND_HINT = (
    "builds, tests, checks and HIL runs run with run_in_background: true; "
    "rerun this command in the background and act on its completion "
    "notification (CLAUDE.md, Rules)"
)


def block(message):
    print(f"Blocked by .claude/hooks/pre_tool_use.py: {message}", file=sys.stderr)
    sys.exit(2)


def strip_heredocs(command):
    """Drop here-document bodies, which are data, not commands."""
    lines = command.split("\n")
    kept, terminator = [], None
    for line in lines:
        if terminator is not None:
            if line.strip() == terminator:
                terminator = None
            continue
        kept.append(line)
        match = re.search(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1", line)
        if match:
            terminator = match.group(2)
    return "\n".join(kept)


def segments(command):
    """Split a shell command into simple commands (lists of words)."""
    command = strip_heredocs(command)
    lexer = shlex.shlex(command.replace("\n", " ; "), posix=True,
                        punctuation_chars=";&|()<>")
    lexer.whitespace_split = True
    lexer.commenters = ""
    try:
        tokens = list(lexer)
    except ValueError:
        tokens = re.findall(r"[;&|()<>]+|[^\s;&|()<>]+", command)
    current = []
    for token in tokens:
        if token and set(token) <= set(";&|()"):
            if current:
                yield current
            current = []
        else:
            current.append(token)
    if current:
        yield current


def unwrap(words):
    """Skip keywords, assignments and wrappers in front of the command."""
    words = list(words)
    while words:
        word = words[0]
        if word in {"{", "}", "!", "then", "do", "else", "elif", "if", "while",
                    "until"} or re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*=.*", word):
            words.pop(0)
            continue
        if os.path.basename(word) in WRAPPERS:
            name = os.path.basename(words.pop(0))
            while words and words[0].startswith("-"):
                flag = words.pop(0)
                if flag in WRAPPER_VALUE_FLAGS and words:
                    words.pop(0)
            if name == "timeout" and words and re.fullmatch(r"[0-9.]+[smhd]?", words[0]):
                words.pop(0)
            if name == "nice" and words and re.fullmatch(r"-?[0-9]+", words[0]):
                words.pop(0)
            continue
        break
    return words


def nested_scripts(words):
    """The script of `sh -c SCRIPT` and similar, analyzed like the outer command."""
    if words and os.path.basename(words[0]) in {"sh", "bash", "zsh", "dash"}:
        for index, word in enumerate(words[1:-1], start=1):
            if word == "-c" or (word.startswith("-") and not word.startswith("--") and "c" in word):
                return [words[index + 1]]
    return []


def subcommand(words):
    """The first word after `cargo` and its global options."""
    rest = words[1:]
    while rest:
        word = rest[0]
        if word.startswith("+"):
            rest = rest[1:]
        elif word in CARGO_VALUE_FLAGS:
            rest = rest[2:]
        elif word.startswith("-"):
            rest = rest[1:]
        else:
            return word, rest[1:]
    return None, []


def first_operand(words):
    for word in words:
        if word in {"--root", "--owner", "--manifest-path"}:
            continue
        if not word.startswith("-"):
            return word
    return None


def heavy_command(words):
    """A description of the foreground-forbidden command, or None."""
    if not words or os.path.basename(words[0]) != "cargo":
        return None
    if any(word in {"--help", "-h", "help", "--version", "-V"} for word in words):
        return None
    sub, rest = subcommand(words)
    if sub in CARGO_HEAVY:
        return f"cargo {sub}"
    operands = [w for i, w in enumerate(rest)
                if not w.startswith("-") and not (i and rest[i - 1] in {"--root", "--owner"})]
    if sub == "xtask" and operands and operands[0].startswith(XTASK_HEAVY):
        if operands[0] == "check" and "--list" in rest:
            return None
        return f"cargo xtask {operands[0]}"
    if sub == "hil" and operands:
        if operands[0] in HIL_HEAVY:
            if operands[0] != "wait" and ("--enqueue" in rest or "--validate-only" in rest):
                return None
            return f"cargo hil {operands[0]}"
        if operands[:2] == ["image", "build"]:
            return "cargo hil image build"
    return None


def kills_by_name(words):
    if not words or os.path.basename(words[0]) not in {"pkill", "pgrep"}:
        return False
    for word in words[1:]:
        if word == "--full" or (re.fullmatch(r"-[A-Za-z]+", word) and "f" in word):
            return True
    return False


def switches_hub_power(words):
    if not words or os.path.basename(words[0]) != "uhubctl":
        return False
    return any(
        word in {"-a", "--action"}
        or word.startswith("--action=")
        or re.fullmatch(r"-a\w+", word) is not None
        for word in words[1:]
    )


def reads_generated(words):
    if not words:
        return None
    name = os.path.basename(words[0])
    candidates = []
    if name in READERS:
        candidates = words[1:]
    elif name == "git" and len(words) > 2 and words[1] in {"show", "cat-file"}:
        candidates = [w.split(":", 1)[-1] for w in words[2:]]
    for word in candidates:
        if GENERATED.search(word.lstrip("<")):
            return word
    return None


def check_bash(tool_input):
    command = tool_input.get("command") or ""
    background = tool_input.get("run_in_background") in (True, "true")
    pending = [command]
    while pending:
        for raw in segments(pending.pop()):
            words = unwrap(raw)
            pending.extend(nested_scripts(words))
            if kills_by_name(words):
                block(f"`{words[0]} -f` matches the waiting shell's own command "
                      "line; wait on a PID (`wait`, `tail --pid`) or a job "
                      "(`cargo hil wait JOB|RUN`)")
            if switches_hub_power(words):
                block("a hub port is switched only by `cargo hil board reset BOARD "
                      "--via power`, a power cycle under the board's lease; "
                      "`uhubctl` without an action only reads the hub")
            path = reads_generated(words)
            if path:
                block(f"`{path}` {GENERATED_HINT}")
            heavy = heavy_command(words)
            if heavy and not background:
                block(f"`{heavy}` in the foreground: {BACKGROUND_HINT}")


def check_read(tool_input):
    path = tool_input.get("file_path") or ""
    if GENERATED.search(path.replace("\\", "/")):
        block(f"`{path}` {GENERATED_HINT}")


def main():
    try:
        event = json.load(sys.stdin)
    except (ValueError, OSError):
        return 0
    tool = event.get("tool_name")
    tool_input = event.get("tool_input") or {}
    if tool == "Bash":
        check_bash(tool_input)
    elif tool == "Read":
        check_read(tool_input)
    return 0


if __name__ == "__main__":
    sys.exit(main())
