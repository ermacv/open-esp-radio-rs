"""Offline regressions for recording pre-existing review findings; no GitHub calls."""

import unittest
import issue_labels
import review_findings as findings


def finding(severity="pre-existing", title="stale cache", path="a/b.rs", **fields):
    return {"severity": severity, "path": path, "line": 7, "title": title, "body": "why",
            "area": "area:tooling", "priority": "priority:P2", "issue": 0, **fields}


class FakeGitHub:
    def __init__(self, issues=()):
        self.issues = list(issues)
        self.created = []

    def list(self, path):
        assert path == "issues?state=all", path
        return self.issues

    def request(self, method, path, data=None):
        if method == "POST" and path == "issues":
            issue = {"number": 500 + len(self.created), "body": data["body"],
                     "user": {"login": "github-actions[bot]"},
                     "labels": [{"name": name} for name in data["labels"]],
                     "html_url": "https://github.com/x/y/issues/500"}
            self.created.append(data)
            self.issues.append(issue)
            return issue
        # create_issue reads the issue back to verify its labels.
        assert method == "GET", (method, path)
        number = int(path.removeprefix("issues/"))
        return next(issue for issue in self.issues if issue["number"] == number)


def filed(number, author="ermacv", **fields):
    """An issue that records the default finding, by `author`."""
    return {"number": number, "title": "t", "state": "open", "user": {"login": author},
            "body": findings.marker(finding()), **fields}


class Record(unittest.TestCase):
    def setUp(self):
        self.catalog = issue_labels.load_catalog()

    def record(self, api, *items):
        return findings.record(api, {"summary": "", "findings": list(items)}, "386", "abc",
                               issue_labels.REPOSITORY, self.catalog)

    def test_only_pre_existing_findings_become_labelled_issues(self):
        api = FakeGitHub()
        lines = self.record(api, finding(severity="nit"), finding(severity="important"), finding())
        self.assertEqual(len(api.created), 1)
        created = api.created[0]
        self.assertEqual(created["title"], "review: stale cache")
        self.assertEqual(created["labels"], ["kind:bug", "area:tooling", "priority:P2"])
        self.assertIn("#386", created["body"])
        self.assertIn(findings.marker(finding()), created["body"])
        self.assertEqual(lines, ["- 🟣 **stale cache**: recorded as #500"])

    def test_a_finding_recorded_before_is_not_filed_again(self):
        api = FakeGitHub()
        self.record(api, finding())
        lines = self.record(api, finding(), finding(line=99, body="reworded"))
        self.assertEqual(len(api.created), 1)
        self.assertEqual(lines, ["- 🟣 **stale cache**: already recorded as #500"] * 2)

    def test_a_finding_the_reviewer_names_as_filed_is_not_filed_again(self):
        api = FakeGitHub([filed(42)])
        lines = self.record(api, finding(title="reworded", issue=42))
        self.assertEqual(api.created, [])
        self.assertEqual(lines, ["- 🟣 **reworded**: already recorded as #42"])

    def test_a_named_number_the_reviewer_was_not_shown_files_the_finding(self):
        api = FakeGitHub([
            filed(41, pull_request={}),
            {"number": 43, "title": "t", "state": "open", "user": {"login": "ermacv"},
             "body": "no marker"},
        ])
        for number in (41, 43, 9999):
            self.record(api, finding(title=f"reworded {number}", issue=number))
        self.assertEqual(len(api.created), 3)

    def test_only_a_trusted_authors_marker_records_a_finding(self):
        api = FakeGitHub([filed(44, title="owner"), filed(42, author="someone")])
        self.assertEqual([issue["number"] for issue in findings.recorded(api).values()], [44])
        api = FakeGitHub([filed(42, author="someone")])
        lines = self.record(api, finding(issue=42))
        self.assertEqual(len(api.created), 1)
        self.assertEqual(lines, ["- 🟣 **stale cache**: recorded as #500"])

    def test_another_path_or_title_is_another_finding(self):
        self.assertNotEqual(findings.key(finding()), findings.key(finding(path="a/c.rs")))
        self.assertNotEqual(findings.key(finding()), findings.key(finding(title="other")))

    def test_an_invalid_classification_files_nothing(self):
        api = FakeGitHub()
        with self.assertRaises(ValueError):
            self.record(api, finding(area="area:nowhere"))
        self.assertEqual(api.created, [])

    def test_no_pre_existing_finding_reads_nothing(self):
        class Untouchable:
            def list(self, path):
                raise AssertionError(path)
        self.assertEqual(self.record(Untouchable(), finding(severity="nit")), [])


if __name__ == "__main__":
    unittest.main()
