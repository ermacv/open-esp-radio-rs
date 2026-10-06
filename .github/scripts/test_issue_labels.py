"""Regressions for creation and live label reconciliation, without GitHub writes."""

from copy import deepcopy
from urllib.error import HTTPError
from urllib.parse import unquote
import unittest

import issue_labels as policy


VALID = ["kind:refactor", "area:tooling", "priority:P2"]


class FakeGitHub:
    def __init__(self, labels=(), body="", state="open"):
        self.issue = {"number": 231, "labels": list(labels), "body": body, "state": state,
                      "html_url": "https://github.com/ermacv/open-esp-radio-rs/issues/231"}
        self.definitions = deepcopy(policy.load_catalog())
        self.calls = []
        self.after_write = None
        self.drop_creation_labels = False

    def list(self, path):
        if path == "labels":
            return list(self.definitions.values())
        if path == "issues?state=open":
            return [deepcopy(self.issue)]
        raise AssertionError(path)

    def request(self, method, path, data=None):
        self.calls.append((method, path, deepcopy(data)))
        if path == "issues" and method == "POST":
            self.issue.update(deepcopy(data))
            if self.drop_creation_labels:
                self.issue["labels"] = []
            return deepcopy(self.issue)
        if path == "issues/231" and method == "GET":
            return deepcopy(self.issue)
        if path == "issues/231/labels" and method == "POST":
            self.issue["labels"] = list(dict.fromkeys(self.issue["labels"] + data["labels"]))
            if self.after_write:
                callback, self.after_write = self.after_write, None
                callback(self.issue)
            return deepcopy(self.issue["labels"])
        if path.startswith("issues/231/labels/") and method == "DELETE":
            self.issue["labels"].remove(unquote(path.split("/")[-1]))
            return None
        if path == "labels" and method == "POST":
            self.definitions[data["name"]] = deepcopy(data)
            return deepcopy(data)
        if path.startswith("labels/") and method in ("GET", "PATCH"):
            name = unquote(path.split("/")[-1])
            if method == "PATCH":
                self.definitions[name].update(data)
            return deepcopy(self.definitions[name])
        raise AssertionError((method, path, data))


class IssueLabelsTests(unittest.TestCase):
    def setUp(self):
        self.catalog = policy.load_catalog()

    def test_executable_kind_area_and_priority_are_required_before_creation(self):
        for labels in [[], ["kind:bug"], VALID[:-1], VALID[1:], [VALID[0], VALID[2]],
                       VALID + ["kind:bug"], VALID + ["priority:P1"], VALID + [VALID[0]],
                       ["kind:unknown", "area:tooling", "priority:P2"],
                       ["kind:bug", "area:unknown", "priority:P2"],
                       ["kind:bug", "area:tooling", "priority:unknown"],
                       VALID + ["target:unknown"], VALID + [policy.INCOMPLETE]]:
            with self.subTest(labels=labels):
                api = FakeGitHub()
                with self.assertRaises(ValueError):
                    policy.create_issue(api, "Result", "Evidence", labels, self.catalog)
                self.assertEqual(api.calls, [])

    def test_tracking_has_the_only_priority_exception(self):
        for labels in [["kind:tracking", "area:tooling"],
                       ["kind:tracking", "area:tooling", "priority:P2"]]:
            self.assertEqual(policy.violations(labels, self.catalog), [])
        self.assertTrue(policy.violations(["kind:tracking"], self.catalog))
        self.assertTrue(policy.violations(
            ["kind:tracking", "area:tooling", "priority:P1", "priority:P2"], self.catalog))

    def test_multiple_areas_targets_and_unmanaged_labels_do_not_conflict(self):
        self.assertEqual(policy.violations(
            VALID + ["area:hil", "target:esp32s31", "target:esp32c5", "good first issue"],
            self.catalog), [])
        self.assertEqual(policy.violations([{"name": name} for name in VALID], self.catalog), [])

    def test_first_creation_request_contains_labels_and_readback_checks_access(self):
        api = FakeGitHub()
        policy.create_issue(api, "Result", "Evidence", VALID, self.catalog)
        self.assertEqual(api.calls[0], ("POST", "issues", {
            "title": "Result", "body": "Evidence", "labels": VALID}))
        self.assertEqual(api.calls[1], ("GET", "issues/231", None))
        api = FakeGitHub()
        api.drop_creation_labels = True
        with self.assertRaisesRegex(RuntimeError, "labels were not accepted"):
            policy.create_issue(api, "Result", "Evidence", VALID, self.catalog)

    def test_unlabeled_api_issue_is_flagged_without_guessing_classification(self):
        api = FakeGitHub(body="Review follow-up, task type: Bug")
        issue, errors = policy.enforce_issue(api, 231, self.catalog, import_form=True)
        self.assertEqual(issue["labels"], [policy.INCOMPLETE])
        self.assertEqual(len(errors), 3)
        before = list(api.calls)
        policy.enforce_issue(api, 231, self.catalog)
        self.assertEqual([call for call in api.calls[len(before):] if call[0] != "GET"], [])

    def test_explicit_form_choices_are_imported_and_the_flag_removed(self):
        body = "### Kind of result\n\nkind:refactor\n\n### Primary area\n\narea:tooling\n\n### Priority\n\npriority:P2\n"
        api = FakeGitHub([policy.INCOMPLETE], body)
        issue, errors = policy.enforce_issue(api, 231, self.catalog, import_form=True)
        self.assertEqual(set(issue["labels"]), set(VALID))
        self.assertEqual(errors, [])

    def test_triage_uses_live_labels_not_old_form_answers_or_event_snapshots(self):
        body = "### Primary area\n\narea:wifi\n\n### Priority\n\npriority:P3\n"
        api = FakeGitHub(VALID + ["area:hil", "good first issue"], body)
        policy.enforce_issue(api, 231, self.catalog, import_form=True)
        self.assertEqual(api.issue["labels"], VALID + ["area:hil", "good first issue"])
        api.issue["labels"].remove("priority:P2")
        issue, errors = policy.enforce_issue(api, 231, self.catalog)
        self.assertTrue(errors)
        self.assertNotIn("priority:P3", issue["labels"])
        self.assertIn(policy.INCOMPLETE, issue["labels"])
        api.issue["labels"].append("priority:P1")
        issue, errors = policy.enforce_issue(api, 231, self.catalog)
        self.assertEqual(errors, [])
        self.assertNotIn(policy.INCOMPLETE, issue["labels"])

    def test_multiline_duplicate_unknown_and_fenced_choices_are_not_inferred(self):
        bodies = ["### Priority\npriority:P2\npriority:P1",
                  "### Primary area\narea:tooling\n### Primary area\narea:wifi",
                  "### Kind of result\nBug", "### Priority\npriority:unknown",
                  "```markdown\n### Priority\npriority:P2\n```",
                  "~~~markdown\n### Priority\npriority:P2\n~~~"]
        for body in bodies:
            with self.subTest(body=body):
                self.assertEqual(policy.form_labels(body, [], self.catalog), [])

    def test_closed_issues_release_flag_and_pull_requests_are_excluded(self):
        api = FakeGitHub([policy.INCOMPLETE], state="closed")
        issue, errors = policy.enforce_issue(api, 231, self.catalog)
        self.assertEqual(issue["labels"], [])
        self.assertEqual(errors, [])
        api = FakeGitHub()
        api.issue["pull_request"] = {}
        self.assertIsNone(policy.enforce_issue(api, 231, self.catalog, import_form=True))
        self.assertTrue(all(call[0] == "GET" for call in api.calls))

    def test_concurrent_classification_is_rechecked_after_flag_write(self):
        api = FakeGitHub()
        api.after_write = lambda issue: issue["labels"].extend(VALID)
        issue, errors = policy.enforce_issue(api, 231, self.catalog)
        self.assertEqual(set(issue["labels"]), set(VALID))
        self.assertEqual(errors, [])

    def test_catalog_sync_repairs_drift_without_touching_unmanaged_labels(self):
        api = FakeGitHub()
        del api.definitions[policy.INCOMPLETE]
        api.definitions["kind:bug"]["color"] = "000000"
        api.definitions["kind:bug"]["description"] = "Changed in UI"
        api.definitions["legacy"] = {"name": "legacy", "color": "123456"}
        policy.sync_catalog(api, self.catalog)
        self.assertEqual(api.definitions["kind:bug"], self.catalog["kind:bug"])
        self.assertEqual(api.definitions[policy.INCOMPLETE], self.catalog[policy.INCOMPLETE])
        self.assertEqual(api.definitions["legacy"], {"name": "legacy", "color": "123456"})
        before = len(api.calls)
        policy.sync_catalog(api, self.catalog)
        self.assertEqual(len(api.calls), before)

    def test_collection_pagination_does_not_skip_issue_101(self):
        api = policy.GitHub("test-token")
        responses = [list(range(100)), [100]]
        paths = []
        def request(method, path):
            self.assertEqual(method, "GET")
            paths.append(path)
            return responses.pop(0)
        api.request = request
        self.assertEqual(api.list("issues?state=open"), list(range(101)))
        self.assertEqual(paths, ["issues?state=open&per_page=100&page=1",
                                 "issues?state=open&per_page=100&page=2"])

    def test_api_permission_failures_propagate(self):
        api = FakeGitHub()
        def rejected(method, path, data=None):
            raise HTTPError(path, 403, "Forbidden", {}, None)
        api.request = rejected
        with self.assertRaises(HTTPError):
            policy.enforce_issue(api, 231, self.catalog)


if __name__ == "__main__":
    unittest.main()
