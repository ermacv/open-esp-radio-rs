"""Issue-label policy, creation preflight and GitHub metadata reconciliation.

Python's standard library only. Labels are canonical after creation; form
answers are imported once, on opened, and never overwrite later triage.
"""

import argparse
import json
import os
from pathlib import Path
import re
import sys
import unittest
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import HTTPRedirectHandler, Request, build_opener


REPOSITORY = "ermacv/open-esp-radio-rs"
CATALOG = Path(__file__).resolve().parents[1] / "labels.json"
INCOMPLETE = "triage:incomplete"
PREFIXES = ("kind:", "area:", "priority:", "target:", "triage:")
FORM_FIELDS = {"Kind of result": "kind:", "Primary area": "area:", "Priority": "priority:"}


def load_catalog(path=CATALOG):
    definitions = json.loads(Path(path).read_text())
    catalog = {}
    for definition in definitions:
        name = definition["name"]
        if (not isinstance(name, str) or not name.startswith(PREFIXES)
                or name in catalog or not re.fullmatch(r"[0-9a-f]{6}", definition["color"])
                or not 1 <= len(definition["description"]) <= 100):
            raise ValueError(f"Invalid or duplicate label definition: {name!r}")
        catalog[name] = definition
    if INCOMPLETE not in catalog:
        raise ValueError(f"Catalog must define {INCOMPLETE}")
    return catalog


def label_names(labels):
    return [label if isinstance(label, str) else label["name"] for label in labels]


def violations(labels, catalog):
    names = label_names(labels)
    errors = []
    if len(names) != len(set(names)):
        errors.append("Duplicate labels are not allowed")
    if any(name.startswith(PREFIXES) and name not in catalog for name in names):
        errors.append("Unknown managed label; use .github/labels.json")
    kinds = [name for name in names if name.startswith("kind:")]
    priorities = [name for name in names if name.startswith("priority:")]
    if len(kinds) != 1:
        errors.append("Exactly one kind:* label is required")
    if not any(name.startswith("area:") for name in names):
        errors.append("At least one area:* label is required")
    if kinds == ["kind:tracking"]:
        if len(priorities) > 1:
            errors.append("A tracking issue permits at most one priority:* label")
    elif len(priorities) != 1:
        errors.append("Exactly one priority:* label is required on an executable issue")
    return errors


def form_labels(body, labels, catalog):
    """Read only unambiguous, explicit canonical choices outside code fences."""
    sections = {}
    heading = None
    fence = None
    for line in (body or "").splitlines():
        match = re.match(r"^ {0,3}(`{3,}|~{3,})", line)
        if match:
            marker = match[1]
            if fence is None:
                fence = marker
            elif marker[0] == fence[0] and len(marker) >= len(fence):
                fence = None
            continue
        if fence is not None:
            continue
        if line.startswith("### "):
            heading = line[4:].strip()
            sections.setdefault(heading, []).append([])
        elif heading is not None:
            sections[heading][-1].append(line)
    names = label_names(labels)
    additions = []
    for heading, prefix in FORM_FIELDS.items():
        values = sections.get(heading, [])
        if len(values) != 1 or any(name.startswith(prefix) for name in names):
            continue
        value = "\n".join(values[0]).strip()
        if value.startswith(prefix) and value in catalog:
            additions.append(value)
    return additions


class RepositoryRedirects(HTTPRedirectHandler):
    def redirect_request(self, request, response, code, message, headers, new_url):
        raise HTTPError(request.full_url, code, "Repository redirects are not followed", headers, response)


class GitHub:
    def __init__(self, token):
        if not token:
            raise ValueError("Set GH_TOKEN or GITHUB_TOKEN with repository Issues write access")
        self.token = token
        self.opener = build_opener(RepositoryRedirects())

    def request(self, method, path, data=None):
        request = Request(
            f"https://api.github.com/repos/{REPOSITORY}/{path}",
            data=None if data is None else json.dumps(data).encode(),
            method=method,
            headers={"Accept": "application/vnd.github+json",
                     "Authorization": f"Bearer {self.token}",
                     "Content-Type": "application/json", "User-Agent": "oer-issue-labels",
                     "X-GitHub-Api-Version": "2026-03-10"},
        )
        with self.opener.open(request, timeout=30) as response:
            body = response.read()
            return json.loads(body) if body else None

    def list(self, path):
        items = []
        page = 1
        separator = "&" if "?" in path else "?"
        while True:
            batch = self.request("GET", f"{path}{separator}per_page=100&page={page}")
            items.extend(batch)
            if len(batch) < 100:
                return items
            page += 1


def sync_catalog(api, catalog):
    live = {label["name"]: label for label in api.list("labels")}
    for name, definition in catalog.items():
        if name not in live:
            try:
                api.request("POST", "labels", definition)
                continue
            except HTTPError as error:
                # Another event can create the same definition concurrently.
                if error.code not in (409, 422):
                    raise
                live[name] = api.request("GET", f"labels/{quote(name, safe='')}")
        if any(live[name].get(key) != definition[key] for key in ("color", "description")):
            api.request("PATCH", f"labels/{quote(name, safe='')}", definition)


def enforce_issue(api, number, catalog, import_form=False):
    path = f"issues/{number}"
    issue = api.request("GET", path)
    if "pull_request" in issue:
        return None
    if import_form and issue["state"] == "open":
        additions = form_labels(issue.get("body"), issue["labels"], catalog)
        if additions:
            api.request("POST", f"{path}/labels", {"labels": additions})
    # Read live labels, not the event's stale snapshot. Check after our write
    # too: GITHUB_TOKEN label changes do not trigger another workflow run.
    for _ in range(3):
        issue = api.request("GET", path)
        errors = violations(issue["labels"], catalog) if issue["state"] == "open" else []
        marked = INCOMPLETE in label_names(issue["labels"])
        if bool(errors) == marked:
            return issue, errors
        if errors:
            api.request("POST", f"{path}/labels", {"labels": [INCOMPLETE]})
        else:
            try:
                api.request("DELETE", f"{path}/labels/{quote(INCOMPLETE, safe='')}")
            except HTTPError as error:
                if error.code != 404:
                    raise
    raise RuntimeError(f"Issue #{number} changed during label reconciliation; rerun the workflow")


def creation_violations(labels, catalog):
    errors = violations(labels, catalog)
    if INCOMPLETE in labels:
        errors.append(f"{INCOMPLETE} is derived by the guard, not chosen at creation")
    return errors


def create_issue(api, title, body, labels, catalog):
    errors = creation_violations(labels, catalog)
    if errors:
        raise ValueError("; ".join(errors))
    issue = api.request("POST", "issues", {"title": title, "body": body, "labels": labels})
    # GitHub can silently drop labels for a token without the necessary access.
    observed = api.request("GET", f"issues/{issue['number']}")
    errors = violations(observed["labels"], catalog)
    if set(labels) - set(label_names(observed["labels"])):
        errors.append("Requested labels are absent from the created issue")
    if errors:
        raise RuntimeError(f"Created {issue['html_url']} but labels were not accepted: {'; '.join(errors)}")
    return observed


def report(results, destination=None):
    invalid = [(issue, errors) for issue, errors in results if errors]
    lines = [f"Issue labels: {len(results)} checked, {len(invalid)} incomplete."]
    for issue, errors in invalid:
        lines.append(f"- [#{issue['number']}]({issue['html_url']}): {'; '.join(errors)}")
    text = "\n".join(lines) + "\n"
    print(text, end="")
    if destination:
        with open(destination, "a") as summary:
            summary.write(text)
    return bool(invalid)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check", help="Validate the catalog and run offline regression tests")
    validate = commands.add_parser("validate", help="Preflight labels before any API/CLI creation")
    validate.add_argument("--label", action="append", required=True)
    create = commands.add_parser("create", help="Create with explicit validated labels in the first request")
    create.add_argument("--title", required=True)
    create.add_argument("--body-file", type=Path, required=True)
    create.add_argument("--label", action="append", required=True)
    commands.add_parser("audit", help="Read-only audit of all open issues; fails on violations")
    commands.add_parser("enforce", help="Reconcile catalog and derived triage flag for a workflow event")
    args = parser.parse_args()
    catalog = load_catalog()
    if args.command == "check":
        suite = unittest.defaultTestLoader.discover(str(Path(__file__).parent), "test_issue_labels.py")
        return 0 if unittest.TextTestRunner().run(suite).wasSuccessful() else 1
    if args.command == "validate":
        errors = creation_violations(args.label, catalog)
        if errors:
            raise ValueError("; ".join(errors))
        print("Issue labels satisfy the creation invariant.")
        return 0
    if os.environ.get("GITHUB_REPOSITORY", REPOSITORY) != REPOSITORY:
        raise ValueError(f"This policy operates only on {REPOSITORY}")
    api = GitHub(os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN"))
    if args.command == "create":
        print(create_issue(api, args.title, args.body_file.read_text(), args.label, catalog)["html_url"])
        return 0
    if args.command == "audit":
        results = [(issue, violations(issue["labels"], catalog))
                   for issue in api.list("issues?state=open") if "pull_request" not in issue]
        return int(report(results))
    event_path = os.environ.get("GITHUB_EVENT_PATH")
    event = json.loads(Path(event_path).read_text()) if event_path else {}
    sync_catalog(api, catalog)
    if "issue" in event:
        result = enforce_issue(api, event["issue"]["number"], catalog, event.get("action") == "opened")
        results = [result] if result else []
    else:
        results = []
        for issue in api.list("issues?state=open"):
            if "pull_request" not in issue:
                results.append(enforce_issue(api, issue["number"], catalog))
    report(results, os.environ.get("GITHUB_STEP_SUMMARY"))
    # An incomplete issue is successfully quarantined metadata, not a code
    # regression on main. API errors and failed reconciliation fail the job.
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, RuntimeError, HTTPError) as error:
        print(f"Issue labels: {error}", file=sys.stderr)
        sys.exit(1)
