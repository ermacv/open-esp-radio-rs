"""Record a Claude review's pre-existing findings as issues.

The publish job of the review workflow runs this from trusted main with the
review's structured result in RESULT. Each 🟣 pre-existing finding becomes one
`kind:bug` issue with the area and priority the reviewer chose, unless an
issue already records it: the body carries a key of the finding's path and
title, so a re-review or a later PR never files it twice. Prints one Markdown
line per finding for the PR comment. Python's standard library only.
"""

import hashlib
import json
import os
import sys

import issue_labels


# The bot that files these issues; only its issues are searched for keys.
CREATOR = "github-actions[bot]"


def key(finding):
    """A stable identity of a finding: its path and title."""
    text = f"{finding['path']}\n{finding['title']}".encode()
    return hashlib.sha256(text).hexdigest()[:16]


def marker(finding):
    return f"<!-- claude-review-finding: {key(finding)} -->"


def labels(finding):
    return ["kind:bug", finding["area"], finding["priority"]]


def body(finding, pr, head, repository):
    url = f"https://github.com/{repository}/blob/{head}/{finding['path']}#L{finding['line']}"
    return "\n".join([
        f"Pre-existing defect found by the Claude review of #{pr} at "
        f"[`{finding['path']}:{finding['line']}`]({url}); it is not part of that "
        "pull request and did not block it.",
        "",
        finding["body"],
        "",
        marker(finding),
    ])


def recorded(api):
    """The issues this workflow filed, open or closed, by finding key."""
    issues = {}
    for issue in api.list(f"issues?state=all&creator={CREATOR}"):
        for line in (issue.get("body") or "").splitlines():
            if line.startswith("<!-- claude-review-finding: ") and line.endswith(" -->"):
                issues[line.removeprefix("<!-- claude-review-finding: ").removesuffix(" -->")] = issue
    return issues


def record(api, result, pr, head, repository, catalog):
    """File an issue for every pre-existing finding not yet recorded; return
    one Markdown line per pre-existing finding naming its issue."""
    findings = [f for f in result["findings"] if f["severity"] == "pre-existing"]
    if not findings:
        return []
    known = recorded(api)
    lines = []
    for finding in findings:
        issue = known.get(key(finding))
        if issue is None:
            issue = issue_labels.create_issue(
                api, f"review: {finding['title']}", body(finding, pr, head, repository),
                labels(finding), catalog)
            known[key(finding)] = issue
            verb = "recorded as"
        else:
            verb = "already recorded as"
        lines.append(f"- 🟣 **{finding['title']}**: {verb} #{issue['number']}")
    return lines


def main():
    result = json.loads(os.environ["RESULT"])
    repository = os.environ["GH_REPO"]
    if repository != issue_labels.REPOSITORY:
        raise ValueError(f"This policy operates only on {issue_labels.REPOSITORY}")
    api = issue_labels.GitHub(os.environ.get("GH_TOKEN"))
    lines = record(api, result, os.environ["PR"], os.environ["HEAD"], repository,
                   issue_labels.load_catalog())
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main())
