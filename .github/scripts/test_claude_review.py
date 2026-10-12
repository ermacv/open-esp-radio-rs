"""Offline regressions for the Agent SDK review script; no API or GitHub calls."""

import io
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import claude_review as review


def result(**fields):
    return SimpleNamespace(**{"subtype": "success", "is_error": False, "api_error_status": None,
                              "result": None, "errors": None, "structured_output": None,
                              "permission_denials": None, **fields})


class IdentityToken(unittest.TestCase):
    def test_writes_the_audience_scoped_token_privately(self):
        requests = []

        def opener(request, timeout):
            requests.append(request)
            return io.BytesIO(json.dumps({"value": "jwt"}).encode())

        env = {"ACTIONS_ID_TOKEN_REQUEST_URL": "https://token.actions.githubusercontent.com/x?a=1",
               "ACTIONS_ID_TOKEN_REQUEST_TOKEN": "request-token"}
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, env):
            path = Path(directory) / "identity"
            path.write_text("stale")
            review.write_identity_token(path, opener)
            self.assertEqual(path.read_text(), "jwt")
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(sorted(p.name for p in Path(directory).iterdir()), ["identity"])
        self.assertTrue(requests[0].full_url.endswith("?a=1&audience=https://api.anthropic.com"))
        self.assertEqual(requests[0].get_header("Authorization"), "Bearer request-token")


class Failure(unittest.TestCase):
    def test_report_is_usable(self):
        self.assertIsNone(review.failure(result(structured_output={"summary": "", "findings": []})))

    def test_api_rejection_names_status_and_message(self):
        rejected = result(is_error=True, api_error_status=400, result="Credit balance too low")
        self.assertEqual(review.failure(rejected), "success, 400: Credit balance too low")

    def test_loop_errors_and_missing_results_fail(self):
        budget = result(subtype="error_max_budget_usd", is_error=True, errors=["budget reached"])
        self.assertEqual(review.failure(budget), "error_max_budget_usd, no API status: budget reached")
        self.assertIn("no message", review.failure(result()))
        self.assertIsNotNone(review.failure(None))

    def test_a_denied_tool_call_fails_the_review(self):
        denial = {"tool_name": "Bash", "tool_use_id": "t1",
                  "tool_input": {"command": "git -C /work/pr diff origin/main...HEAD"}}
        report = {"summary": "Not verified.", "findings": []}
        self.assertEqual(review.failure(result(structured_output=report, permission_denials=[denial])),
                         "tool calls were denied: Bash: git -C /work/pr diff origin/main...HEAD")
        self.assertIsNone(review.failure(result(structured_output=report, permission_denials=[])))


class Policy(unittest.TestCase):
    def test_only_read_only_commands_run_without_a_prompt(self):
        commands = [t for t in review.ALLOWED_TOOLS if t.startswith("Bash(")]
        self.assertTrue(all(t.startswith(("Bash(git -C pr ", "Bash(gh pr ", "Bash(gh issue view"))
                            for t in commands), commands)
        self.assertNotIn("Bash", review.ALLOWED_TOOLS)

    def test_the_prompt_names_every_allowed_command_and_the_inputs(self):
        policy = review.tool_policy()
        for tool in review.ALLOWED_TOOLS:
            if tool.startswith("Bash("):
                self.assertIn(f"`{tool[5:-1]}`", policy)
        self.assertIn(f"{review.INPUTS}/", policy)

    def test_schema_requires_every_finding_field(self):
        finding = review.SCHEMA["properties"]["findings"]["items"]
        self.assertEqual(set(finding["required"]), set(finding["properties"]))

    def test_wording_is_a_severity_apart_from_a_nit(self):
        severity = review.SCHEMA["properties"]["findings"]["items"]["properties"]["severity"]
        self.assertEqual(severity["enum"], ["important", "nit", "wording", "pre-existing"])
        self.assertIn("text that misstates behavior is never wording", severity["description"])

    def test_the_prompt_lists_each_filed_finding_once(self):
        issue = {"number": 7, "state": "closed", "title": "review: stale cache"}
        listed = review.filed_findings({"k1": issue, "k2": issue})
        self.assertEqual(listed, "Findings already filed as issues:\n"
                                 "- #7 (closed): review: stale cache")
        self.assertIn("yet", review.filed_findings({}))

    def test_findings_are_classified_by_the_label_catalog(self):
        finding = review.SCHEMA["properties"]["findings"]["items"]["properties"]
        self.assertIn("area:tooling", finding["area"]["enum"])
        self.assertEqual(finding["priority"]["enum"],
                         ["priority:P0", "priority:P1", "priority:P2", "priority:P3"])


if __name__ == "__main__":
    unittest.main()
