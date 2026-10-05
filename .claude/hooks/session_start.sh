#!/bin/sh
# SessionStart context for Claude Code: branch and upstream state, CI on main
# (when oer-xtask is already built), free disk space and, when oer-tidy is
# already built, the tidy checks. Prints at most a few lines; Claude Code adds
# them to the session's context. POSIX sh with git, df, awk and, if present,
# timeout and cargo.

root=${CLAUDE_PROJECT_DIR:-$(pwd)}
cd "$root" 2>/dev/null || exit 0

# Run a command under `timeout` when the host has it.
bounded() {
    seconds=$1
    shift
    if command -v timeout >/dev/null 2>&1; then
        timeout "$seconds" "$@"
    else
        "$@"
    fi
}

# Branch and upstream, after a bounded, non-interactive fetch of that branch.
branch=$(git rev-parse --abbrev-ref HEAD 2>/dev/null)
if [ -n "$branch" ]; then
    upstream=$(git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null)
    if [ -z "$upstream" ]; then
        echo "git: on $branch, no upstream"
    else
        remote=$(git config "branch.$branch.remote")
        merge=$(git config "branch.$branch.merge")
        if GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="ssh -o BatchMode=yes" \
            bounded 5 git fetch --quiet "$remote" "$merge" >/dev/null 2>&1; then
            freshness="fetched now"
        else
            freshness="upstream not checked"
        fi
        counts=$(git rev-list --left-right --count "HEAD...$upstream" 2>/dev/null)
        ahead=${counts%%[[:space:]]*}
        behind=${counts##*[[:space:]]}
        line="git: on $branch, $behind behind and $ahead ahead of $upstream ($freshness)"
        if [ "${behind:-0}" -gt 0 ] 2>/dev/null; then
            line="$line; rebase before pushing"
        fi
        echo "$line"
    fi
fi

# CI on main: each workflow of the tree whose newest finished verdict on main
# failed. Fixing a red main comes before other work. The one reader of CI's
# state is `cargo xtask ci-status`; a hook cannot wait for its build, so the
# executable an earlier build left runs directly, and a checkout that never
# built xtask is told how to ask.
target=${CARGO_TARGET_DIR:-$root/target}
case $target in /*) ;; *) target=$root/$target ;; esac
if [ -x "$target/debug/oer-xtask" ]; then
    output=$(bounded 15 "$target/debug/oer-xtask" --root "$root" ci-status 2>&1)
    if [ -n "$output" ]; then
        echo "$output"
    else
        echo "ci: state of main unknown (no answer within 15 s)"
    fi
else
    echo "ci: state of main unknown; \`cargo xtask ci-status\` (background) builds and asks"
fi

# Free disk space of the checkout's file system.
threshold=${OER_DISK_WARN_GIB:-20}
free_kib=$(df -Pk "$root" 2>/dev/null | awk 'NR == 2 { print $4 }')
if [ -n "$free_kib" ]; then
    free_gib=$((free_kib / 1048576))
    if [ "$free_gib" -lt "$threshold" ]; then
        echo "disk: WARNING only $free_gib GiB free (below $threshold GiB); \`cargo hil sweep\` lists rebuildable caches"
    else
        echo "disk: $free_gib GiB free"
    fi
fi

# Tidy, only when its binary exists: a cold build takes a while, a warm run
# about two seconds. A busy build directory or a slow run is skipped.
if [ -x "$target/debug/oer-tidy" ] && command -v cargo >/dev/null 2>&1; then
    output=$(bounded 30 cargo tidy check 2>&1)
    status=$?
    if [ "$status" -eq 0 ]; then
        echo "$output" | tail -n 1
    elif [ "$status" -eq 124 ]; then
        echo "tidy: skipped (no result within 30 s; the build directory may be busy)"
    else
        echo "tidy: FAILED; run \`cargo tidy check\` in the background. First lines:"
        echo "$output" | grep -v ': ok$' | head -n 6
    fi
else
    echo "tidy: not built yet; \`cargo tidy check\` (background) builds and runs it"
fi
exit 0
