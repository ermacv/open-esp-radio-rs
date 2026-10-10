"""Record a Claude review's findings as issues.

The publish job of the review workflow runs this from trusted main with the
review's structured result in RESULT. The review never blocks a merge, so the
issues are the record of what it found: each finding, 🔴 important, 🟡 nit or
🟣 pre-existing, becomes one `kind:bug` issue with the area and priority the
reviewer chose and the reviewed branch, which `cargo xtask ci-status` lists to
the session on that branch, unless an issue already records it: the reviewer,
shown the findings a trusted author filed, names the issue of one it finds
again, and the body's key of the finding's path and title catches an
identical one. A finding whose issue was closed as fixed came back and is
filed anew; one closed as not planned stays dismissed. Prints one Markdown
line per finding for the PR comment.
Python's standard library only.
"""

import hashlib
import json
import os
import sys

import issue_labels

# The authors whose marked issues record a finding: the workflow itself and
# the repository owner, who files and marks findings by hand. Anyone else's
# marker would suppress a finding and put its title into the review prompt.
TRUSTED = ("github-actions[bot]", issue_labels.REPOSITORY.split("/")[0])

def key(finding):
    """A stable identity of a finding: its path and title."""
    text = f"{finding['path']}\n{finding['title']}".encode()
    return hashlib.sha256(text).hexdigest()[:16]


def marker(finding):
    return f"<!-- claude-review-finding: {key(finding)} -->"


def labels(finding):
    return ["kind:bug", finding["area"], finding["priority"]]


MARKS = {"important": "🔴", "nit": "🟡", "pre-existing": "🟣"}


def branch_marker(branch):
    return f"<!-- claude-review-branch: {branch} -->"


def body(finding, pr, head, repository, branch):
    url = f"https://github.com/{repository}/blob/{head}/{finding['path']}#L{finding['line']}"
    place = f"[`{finding['path']}:{finding['line']}`]({url})"
    if finding["severity"] == "pre-existing":
        origin = (f"Pre-existing defect found by the Claude review of #{pr} at {place}; "
                  "it is not part of that pull request.")
    else:
        origin = (f"{MARKS[finding['severity']]} {finding['severity']} finding of the Claude review "
                  f"of #{pr} at {place}: a defect that pull request introduces or touches. "
                  "The review does not block merging; the session on its branch fixes it next.")
    return "\n".join([origin, "", finding["body"], "", marker(finding), branch_marker(branch)])


def recorded(api):
    """The issues that record a review finding, open or closed, by finding
    key: those of a trusted author that carry the marker."""
    issues = {}
    for issue in api.list("issues?state=all"):
        if "pull_request" in issue or (issue.get("user") or {}).get("login") not in TRUSTED:
            continue
        for line in (issue.get("body") or "").splitlines():
            if line.startswith("<!-- claude-review-finding: ") and line.endswith(" -->"):
                issues[line.removeprefix("<!-- claude-review-finding: ").removesuffix(" -->")] = issue
    return issues


def record(api, result, pr, head, repository, catalog, branch):
    """File an issue for every finding not yet recorded; return one Markdown
    line per finding naming its issue."""
    findings = result["findings"]
    if not findings:
        return []
    known = recorded(api)
    lines = []
    for finding in findings:
        # The reviewer names the issue of a finding it was shown as filed;
        # an identical path and title is caught too. A number it was not
        # shown, such as a pull request or an unmarked issue, files anew.
        filed = {issue["number"]: issue for issue in known.values()}
        issue = known.get(key(finding)) or filed.get(finding["issue"])
        # An issue closed as fixed records a defect that came back, which no
        # open issue would show; one closed as not planned was dismissed.
        regressed = (issue is not None and issue.get("state") == "closed"
                     and issue.get("state_reason") != "not_planned")
        if issue is None or regressed:
            text = body(finding, pr, head, repository, branch)
            if regressed:
                text = f"Reported again after #{issue['number']} was closed as fixed.\n\n{text}"
            issue = issue_labels.create_issue(
                api, f"review: {finding['title']}", text, labels(finding), catalog)
            known[key(finding)] = issue
            verb = "recorded as"
        else:
            verb = "already recorded as"
        lines.append(f"- {MARKS[finding['severity']]} **{finding['title']}**: {verb} #{issue['number']}")
    return lines


def main():
    result = json.loads(os.environ["RESULT"])
    repository = os.environ["GH_REPO"]
    if repository != issue_labels.REPOSITORY:
        raise ValueError(f"This policy operates only on {issue_labels.REPOSITORY}")
    api = issue_labels.GitHub(os.environ.get("GH_TOKEN"))
    lines = record(api, result, os.environ["PR"], os.environ["HEAD"], repository,
                   issue_labels.load_catalog(), os.environ["BRANCH"])
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main())
