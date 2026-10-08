"""Offline regressions for verdicts, immutable reads and event races."""

import base64
import io
import json
import os
from copy import deepcopy
import unittest
from unittest.mock import patch

import claude_review as review


PR = {"number": 7, "state": "open", "draft": False, "title": "Fix polling",
      "body": "Fixes #12", "head": {"sha": "a" * 40, "repo": {"full_name": review.REPOSITORY}},
      "base": {"sha": "b" * 40, "ref": "main"},
      "changed_files": 1, "review_files": [{"filename": "src/lib.rs", "status": "modified"}]}
REPORT = {"summary": "Проверены изменения и вызывающий код.", "coverage_gaps": [], "findings": []}
FINDING = {"path": "src/lib.rs", "line": 8, "priority": "P1", "title": "Lost wakeup",
           "trigger": "IRQ fires between the check and registration",
           "impact": "Future remains pending", "fix": "Register before checking the state"}


class FakeGitHub:
    def __init__(self):
        self.pr = deepcopy(PR)
        self.comments = []
        self.writes = []
        self.runs = []
        self.files = [{"filename": "src/lib.rs", "status": "modified", "additions": 1,
                       "deletions": 1, "patch": "@@ -8 +8 @@\n-old\n+new"}]

    def request(self, method, path, data=None):
        if method != "GET":
            self.writes.append((method, path, deepcopy(data)))
            return {}
        if path == "pulls/7":
            return deepcopy(self.pr)
        if path.startswith("actions/workflows/ci.yml/runs?"):
            return {"workflow_runs": deepcopy(self.runs)}
        if path.startswith("contents/"):
            return {"type": "file", "encoding": "base64",
                    "content": base64.b64encode(b"first\nsecond\nthird\n").decode()}
        raise AssertionError(path)

    def list(self, path):
        if path == "issues/7/comments":
            return deepcopy(self.comments)
        if path == "pulls/7/files":
            return deepcopy(self.files)
        if path.startswith("commits/"):
            return [deepcopy(self.pr), {**deepcopy(self.pr), "number": 8,
                                       "head": {"sha": "c" * 40}}]
        raise AssertionError(path)

    def closing_issues(self, number):
        return {(review.REPOSITORY, 13)}

    def read_issue(self, repo, number):
        return {"repository": repo, "number": number, "title": "Wake correctly", "comments": []}


class ReviewTests(unittest.TestCase):
    def test_approval_requires_complete_analysis_and_successful_ci(self):
        for ci in (None, False):
            self.assertIn("Проверка неполная", review.render(PR, REPORT, ci, "CI pending"))
        incomplete = {**REPORT, "coverage_gaps": ["Missing issue acceptance criteria"]}
        self.assertIn("Проверка неполная", review.render(PR, incomplete, True, "CI"))
        self.assertIn("можно мерджить", review.render(PR, REPORT, True, "CI"))
        failed = {**REPORT, "findings": [FINDING]}
        text = review.render(PR, failed, True, "CI")
        self.assertIn("нужны исправления", text)
        self.assertIn(f"/blob/{PR['head']['sha']}/src/lib.rs#L8", text)
        self.assertNotIn("можно мерджить", text)

    def test_newer_pending_or_failed_ci_rerun_overrides_previous_success(self):
        api = FakeGitHub()
        successful = {"run_number": 10, "run_attempt": 1, "status": "completed",
                      "conclusion": "success", "html_url": "https://example/ci"}
        api.runs = [successful]
        self.assertIs(review.ci_state(api, PR["head"]["sha"])[0], True)
        api.runs.append({**successful, "run_attempt": 2, "status": "in_progress"})
        self.assertIsNone(review.ci_state(api, PR["head"]["sha"])[0])
        api.runs[-1].update(status="completed", conclusion="failure")
        self.assertIs(review.ci_state(api, PR["head"]["sha"])[0], False)

    def test_stale_head_base_requirements_or_closed_pr_never_get_a_verdict(self):
        mutations = [{"head": {"sha": "c" * 40}}, {"base": {"sha": "c" * 40}},
                     {"state": "closed"}, {"draft": True}, {"body": "New requirements"}]
        for mutation in mutations:
            api = FakeGitHub()
            api.pr.update(mutation)
            self.assertFalse(review.publish(api, PR, "green"))
            self.assertEqual(api.writes, [])

    def test_one_owned_comment_is_updated_and_forged_markers_are_ignored(self):
        api = FakeGitHub()
        api.comments = [{"id": 1, "user": {"login": "attacker"}, "body": review.MARKER}]
        self.assertTrue(review.publish(api, PR, "pending"))
        self.assertEqual(api.writes[0][0:2], ("POST", "issues/7/comments"))
        api.comments.append({"id": 2, "user": {"login": "github-actions[bot]"}, "body": review.MARKER})
        review.publish(api, PR, "complete")
        self.assertEqual(api.writes[-1][0:2], ("PATCH", "issues/comments/2"))

    def test_local_cross_repository_and_closing_issue_links_are_read(self):
        refs = review.linked_issues("Fixes #12, ermacv/another#5 and https://github.com/x/y/issues/9")
        self.assertEqual(refs, {(review.REPOSITORY, 12), ("ermacv/another", 5), ("x/y", 9)})
        text, _, gaps = review.context(FakeGitHub(), PR)
        self.assertIn('"number": 12', text)
        self.assertIn('"number": 13', text)
        self.assertEqual(gaps, [])

    def test_missing_diff_is_a_coverage_gap_and_missing_files_fail_closed(self):
        api = FakeGitHub()
        del api.files[0]["patch"]
        _, _, gaps = review.context(api, PR)
        self.assertEqual(gaps, ["Недоступен diff: src/lib.rs"])
        api.files = []
        with self.assertRaisesRegex(ValueError, "неполный список"):
            review.context(api, PR)

    def test_truncated_patch_cannot_be_treated_as_complete(self):
        api = FakeGitHub()
        api.files[0]["additions"] = 100
        _, _, gaps = review.context(api, PR)
        self.assertEqual(gaps, ["Diff обрезан: src/lib.rs"])

    def test_source_reads_have_numbered_lines_and_reject_runner_paths(self):
        sources = review.Sources(FakeGitHub(), PR)
        self.assertIn('"lines": ["2: second"]', sources.read("src/lib.rs", "head", 2, 1))
        for path in ("../.env", "/proc/self/environ", "src/../../.env"):
            with self.assertRaises(ValueError):
                sources.read(path, "head")
        with self.assertRaises(ValueError):
            sources.read("crates/hardware/chip/pac/raw/src/lib.rs", "head")
        with self.assertRaises(ValueError):
            sources.read("src/lib.rs", "main")

    def test_findings_require_changed_path_and_concrete_execution_evidence(self):
        for bad in [{**FINDING, "path": "unrelated.rs"}, {**FINDING, "line": 0},
                    {**FINDING, "trigger": ""}, {**FINDING, "impact": ""}]:
            with self.assertRaises(ValueError):
                review.validate_report({**REPORT, "findings": [bad]}, PR["review_files"])

    def test_ci_event_does_not_review_stale_or_other_repository_revisions(self):
        event = {"workflow_run": {"head_repository": {"full_name": review.REPOSITORY},
                                  "head_sha": PR["head"]["sha"]}}
        self.assertEqual(review.targets(FakeGitHub(), event, "workflow_run"), [7])
        event["workflow_run"]["head_repository"]["full_name"] = "x/y"
        self.assertEqual(review.targets(FakeGitHub(), event, "workflow_run"), [])

    def test_waiting_for_ci_posts_pending_without_spending_api_credits(self):
        api = FakeGitHub()
        with patch.object(review, "Claude") as claude:
            review.review_pr(api, 7, review.MODEL)
        claude.assert_not_called()
        comments = [w for w in api.writes if w[1] == "issues/7/comments"]
        self.assertIn("ожидает CI", comments[-1][2]["body"])
        self.assertEqual([w[2]["state"] for w in api.writes if w[1].startswith("statuses/")], ["pending"])

    def test_fork_and_non_main_pull_requests_are_not_reviewed(self):
        for changed in ("head", "base"):
            api = FakeGitHub()
            if changed == "head":
                api.pr["head"]["repo"]["full_name"] = "someone/fork"
            else:
                api.pr["base"]["ref"] = "feature"
            with patch.object(review, "Claude") as claude:
                review.review_pr(api, 7, review.MODEL)
            claude.assert_not_called()
            self.assertEqual(api.writes, [])

    def test_api_failure_replaces_previous_verdict_with_incomplete(self):
        api = FakeGitHub()
        api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                     "conclusion": "success", "html_url": "https://example/ci"}]
        with patch.object(review, "Claude", side_effect=ValueError("Federation not configured")):
            with self.assertRaises(ValueError):
                review.review_pr(api, 7, review.MODEL)
        comments = [w for w in api.writes if w[1] == "issues/7/comments"]
        self.assertIn("Проверка неполная", comments[-1][2]["body"])
        self.assertEqual(api.writes[-1][2]["state"], "error")

    def test_commit_status_only_succeeds_for_complete_clean_analysis_and_ci(self):
        reports = [REPORT, {**REPORT, "findings": [FINDING]},
                   {**REPORT, "coverage_gaps": ["Unreadable source"]}]
        for report, expected in zip(reports, ["success", "failure", "failure"]):
            api = FakeGitHub()
            api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                         "conclusion": "success", "html_url": "https://example/ci"}]
            with patch.object(review, "Claude") as claude:
                claude.return_value.review.return_value = deepcopy(report)
                review.review_pr(api, 7, review.MODEL)
            self.assertEqual(api.writes[-1][1], f"statuses/{PR['head']['sha']}")
            self.assertEqual(api.writes[-1][2]["state"], expected)

    def test_token_budget_stops_before_a_paid_messages_request(self):
        claude = review.Claude(review.MODEL)
        with patch.object(claude, "request", return_value={"input_tokens": review.MAX_INPUT + 1}) as request:
            with self.assertRaisesRegex(ValueError, "лимит токенов"):
                claude.review("context", None, [])
        self.assertEqual([c.args[0] for c in request.call_args_list], ["messages/count_tokens"])

    def test_oidc_exchange_caches_tokens_and_refreshes_with_a_new_assertion(self):
        credentials = review.GitHubOIDC()
        env = {"ANTHROPIC_FEDERATION_RULE_ID": "fdrl_test",
               "ANTHROPIC_ORGANIZATION_ID": "org-test",
               "ANTHROPIC_SERVICE_ACCOUNT_ID": "svac_test",
               "ANTHROPIC_WORKSPACE_ID": "wrkspc_test",
               "ACTIONS_ID_TOKEN_REQUEST_URL": "https://vstoken.actions.githubusercontent.com/token?run=1",
               "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "github-job-credential"}
        responses = [{"value": "assertion-one"}, {"access_token": "token-one", "expires_in": 600},
                     {"value": "assertion-two"}, {"access_token": "token-two", "expires_in": 600}]
        requests = []
        def opened(request, **kwargs):
            requests.append(request)
            return io.BytesIO(json.dumps(responses.pop(0)).encode())
        with patch.dict(os.environ, env, clear=True), patch.object(credentials.opener, "open", side_effect=opened):
            self.assertEqual(credentials.authorization(), "Bearer token-one")
            self.assertEqual(credentials.authorization(), "Bearer token-one")
            self.assertEqual(len(requests), 2)
            credentials.refresh_at = 0
            self.assertEqual(credentials.authorization(), "Bearer token-two")
        self.assertIn("audience=https%3A%2F%2Fapi.anthropic.com", requests[0].full_url)
        first, second = json.loads(requests[1].data), json.loads(requests[3].data)
        self.assertEqual(first["grant_type"], "urn:ietf:params:oauth:grant-type:jwt-bearer")
        self.assertEqual(first["workspace_id"], "wrkspc_test")
        self.assertEqual(first["assertion"], "assertion-one")
        self.assertEqual(second["assertion"], "assertion-two")

    def test_oidc_does_not_silently_use_a_static_api_key(self):
        with patch.dict(os.environ, {"ANTHROPIC_API_KEY": "leftover-key"}, clear=True):
            with self.assertRaisesRegex(ValueError, "OIDC federation"):
                review.GitHubOIDC().authorization()


if __name__ == "__main__":
    unittest.main()
