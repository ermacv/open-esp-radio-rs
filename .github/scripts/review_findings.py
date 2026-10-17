"""Record a Claude review's findings as issues.

The publish job of the review workflow runs this from trusted main with the
review's structured result in RESULT; `cargo xtask push` runs it with the
wording findings of the local review, which do not block the push, and, when
a round's second local pass still blocks, its blocking findings too. The CI
review sets no required status, so the issues are the record of what it
found; an open 🔴 or 🟡 finding blocks the pull request's readiness, while a
🔵 one rides along or stays filed (docs/issues.md, review and readiness):
each finding, 🔴 important, 🟡 nit, 🔵 wording or 🟣 pre-existing, becomes one
issue (`kind:docs` for wording, `kind:bug` otherwise) with the area and
priority the reviewer chose and the reviewed branch, which `cargo xtask ci-status` lists to
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


def marker(finding, found=None):
    """The body line that records `finding`, under the key `found` of the
    finding it repeats, if any."""
    return f"<!-- claude-review-finding: {found or key(finding)} -->"


def labels(finding):
    """The labels of a finding's issue: a wording finding asks to improve
    correct text, every other one reports a defect."""
    kind = "kind:docs" if finding["severity"] == "wording" else "kind:bug"
    return [kind, finding["area"], finding["priority"]]


# What each severity means, as the report schemas describe it (the local
# review's copy is `oer_xtask::review::SEVERITY`): the line between a nit and
# wording is whether the text is correct.
SEVERITY = ("important: a reachable failure or a violated contract, with its trigger. "
            "nit: a real but minor defect the change introduces or touches, text that "
            "misstates behavior included (documentation, comments, messages, names that say "
            "something false). wording: only the phrasing, naming or clarity of text that is "
            "correct; text that misstates behavior is never wording. pre-existing: a defect "
            "outside the change. An important or nit finding blocks the pull request's "
            "readiness; a wording one rides along or is filed.")

MARKS = {"important": "🔴", "nit": "🟡", "wording": "🔵", "pre-existing": "🟣"}


def branch_marker(branch):
    return f"<!-- claude-review-branch: {branch} -->"


def body(finding, pr, head, repository, branch, found=None):
    url = f"https://github.com/{repository}/blob/{head}/{finding['path']}#L{finding['line']}"
    place = f"[`{finding['path']}:{finding['line']}`]({url})"
    if finding["severity"] == "pre-existing":
        origin = (f"Pre-existing defect found by the Claude review of #{pr} at {place}; "
                  "it is not part of that pull request.")
    elif finding["severity"] == "wording":
        origin = (f"{MARKS['wording']} wording finding of the Claude review of #{pr} at {place}: "
                  "the phrasing, naming or clarity of correct text that pull request "
                  "introduces or touches. It does not block readiness: it rides along or "
                  "the session on its branch improves it next.")
    else:
        origin = (f"{MARKS[finding['severity']]} {finding['severity']} finding of the Claude review "
                  f"of #{pr} at {place}: a defect that pull request introduces or touches. "
                  "While it is open it blocks the pull request's readiness "
                  "([review and readiness](https://github.com/ermacv/open-esp-radio-rs/issues/714#issuecomment-6231604828)); the session on its branch fixes it next.")
    return "\n".join([origin, "", finding["body"], "", marker(finding, found),
                      branch_marker(branch)])


# How a person closes an issue to dismiss its finding rather than fix it.
DISMISSED = ("not_planned", "duplicate")


def marked(api):
    """Every issue that records a review finding, open or closed, with its
    finding key: those of a trusted author that carry the marker."""
    for issue in api.list("issues?state=all"):
        if "pull_request" in issue or (issue.get("user") or {}).get("login") not in TRUSTED:
            continue
        for line in (issue.get("body") or "").splitlines():
            if line.startswith("<!-- claude-review-finding: ") and line.endswith(" -->"):
                yield line.removeprefix("<!-- claude-review-finding: ").removesuffix(" -->"), issue


def current(issues):
    """The issue that speaks for a finding filed more than once: an open one,
    else the newest, since a finding is filed again after its fix regressed."""
    return max(issues, key=lambda issue: (issue.get("state") != "closed", issue["number"]))


def recorded_from(pairs):
    """The issue that records each review finding, by finding key, of the
    (key, issue) pairs `marked` yields."""
    by_key = {}
    for found, issue in pairs:
        by_key.setdefault(found, []).append(issue)
    return {found: current(issues) for found, issues in by_key.items()}


def recorded(api):
    """The issue that records each review finding, by finding key."""
    return recorded_from(marked(api))


def record(api, result, pr, head, repository, catalog, branch):
    """File an issue for every finding not yet recorded; return one Markdown
    line per finding naming its issue."""
    findings = result["findings"]
    if not findings:
        return []
    pairs = list(marked(api))
    known = recorded_from(pairs)
    # Any number filed for a finding, the closed original of a refiled one
    # included, leads to the finding's current issue.
    keys = {issue["number"]: found for found, issue in pairs}
    lines = []
    for finding in findings:
        # The reviewer names the issue of a finding it was shown as filed;
        # an identical path and title is caught too. A number it was not
        # shown, such as a pull request or an unmarked issue, files anew.
        found = key(finding) if key(finding) in known else keys.get(finding["issue"])
        issue = known.get(found)
        # An issue closed as fixed records a defect that came back, which no
        # open issue would show; one dismissed stays dismissed.
        regressed = (issue is not None and issue.get("state") == "closed"
                     and issue.get("state_reason") not in DISMISSED)
        if issue is None or regressed:
            # A refiled finding keeps its key, so its new issue speaks for it.
            text = body(finding, pr, head, repository, branch, found if regressed else None)
            if regressed:
                text = f"Reported again after #{issue['number']} was closed as fixed.\n\n{text}"
            issue = issue_labels.create_issue(
                api, f"review: {finding['title']}", text, labels(finding), catalog)
            found = found if regressed else key(finding)
            known[found] = issue
            keys[issue["number"]] = found
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
