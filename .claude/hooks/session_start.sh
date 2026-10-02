#!/bin/sh
# SessionStart context for Claude Code: branch and upstream state, free disk
# space and, when oer-tidy is already built, the tidy checks. Prints at most
# a few lines; Claude Code adds them to the session's context. POSIX sh with
# git, df, awk and, if present, timeout and cargo.

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
# failed. Fixing a red main comes before other work.
if command -v gh >/dev/null 2>&1 && command -v jq >/dev/null 2>&1; then
    workflows=$(sed -n 's/^name:[[:space:]]*//p' .github/workflows/*.yml 2>/dev/null | tr -d "'\"")
    runs=$(bounded 5 gh run list --branch main --limit 30 \
        --json workflowName,status,conclusion,createdAt,url 2>/dev/null)
    if [ -n "$runs" ]; then
        red=$(printf '%s' "$runs" | jq -r --arg workflows "$workflows" '
            ($workflows | split("\n")) as $names
            | [.[] | select(.status == "completed")
                   | select(.conclusion == "success" or .conclusion == "failure"
                            or .conclusion == "timed_out" or .conclusion == "startup_failure")
                   | select(.workflowName as $n | $names | index($n))]
            | group_by(.workflowName)
            | map(max_by(.createdAt))
            | map(select(.conclusion != "success"))
            | .[] | "\(.workflowName) \(.url)"')
        if [ -n "$red" ]; then
            echo "$red" | while read -r name url; do
                echo "ci: main is RED: $name failed ($url); fix it before other work"
            done
        else
            echo "ci: main is green"
        fi
    else
        echo "ci: state of main unknown (gh gave no answer within 5 s)"
    fi
fi

# Free disk space of the checkout's file system.
threshold=${OER_DISK_WARN_GIB:-20}
free_kib=$(df -Pk "$root" 2>/dev/null | awk 'NR == 2 { print $4 }')
if [ -n "$free_kib" ]; then
    free_gib=$((free_kib / 1048576))
    if [ "$free_gib" -lt "$threshold" ]; then
        echo "disk: WARNING only $free_gib GiB free (below $threshold GiB); \`cargo xtask sweep\` lists rebuildable caches"
    else
        echo "disk: $free_gib GiB free"
    fi
fi

# Tidy, only when its binary exists: a cold build takes a while, a warm run
# about two seconds. A busy build directory or a slow run is skipped.
target=${CARGO_TARGET_DIR:-$root/target}
case $target in /*) ;; *) target=$root/$target ;; esac
if [ -x "$target/debug/oer-tidy" ] && command -v cargo >/dev/null 2>&1; then
    output=$(bounded 30 cargo tidy check 2>&1)
    status=$?
    if [ "$status" -eq 0 ]; then
        echo "$output" | tail -n 1
    elif [ "$status" -eq 124 ]; then
        echo "tidy: skipped (no result within 30 s; the build directory may be busy)"
    else
        echo "tidy: FAILED; run \`cargo xtask check tidy\` in the background. First lines:"
        echo "$output" | grep -v ': ok$' | head -n 6
    fi
else
    echo "tidy: not built yet; \`cargo xtask check tidy\` (background) builds and runs it"
fi
exit 0
