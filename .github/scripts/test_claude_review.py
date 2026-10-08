"""Offline regressions for verdicts, immutable reads and event races."""

import base64
import io
import json
import os
from pathlib import Path
import re
from copy import deepcopy
import unittest
from unittest.mock import patch

import claude_review as review


PR = {"number": 7, "state": "open", "draft": False, "title": "Fix polling",
      "body": "Fixes #12; relates to #14", "head": {"sha": "a" * 40, "repo": {"full_name": review.REPOSITORY}},
      "base": {"sha": "b" * 40, "ref": "main"},
      "merge_base_sha": "c" * 40,
      "changed_files": 1, "review_files": [{"filename": "src/lib.rs", "status": "modified"}]}
REPORT = {"summary": "Проверены изменения и вызывающий код.", "coverage_gaps": [],
          "findings": [], "out_of_scope_findings": []}
FINDING = {"path": "src/lib.rs", "line": 8, "priority": "P1", "title": "Lost wakeup",
           "trigger": "IRQ fires between the check and registration",
           "impact": "Future remains pending", "fix": "Register before checking the state"}
UNRELATED = {"path": "src/old.rs", "line": 2, "base_path": "src/old.rs", "base_line": 2,
             "title": "Старый сбой", "trigger": "Независимый старый путь",
             "impact": "Потеря события", "reason": "Поведение и достижимость не меняются в PR"}


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
        if path.startswith("compare/"):
            return {"merge_base_commit": {"sha": self.pr["merge_base_sha"]}}
        if path.startswith("actions/workflows/ci.yml/runs?"):
            return {"workflow_runs": deepcopy(self.runs)}
        if path.startswith("contents/"):
            return {"type": "file", "encoding": "base64",
                    "content": base64.b64encode(b"first\nsecond\nthird\n4\n5\n6\n7\n8\n9\n10\n").decode()}
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
        return {(review.REPOSITORY, 12), (review.REPOSITORY, 13)}

    def read_issue(self, repo, number):
        return {"repository": repo, "number": number, "title": "Wake correctly", "comments": []}


class ReviewTests(unittest.TestCase):
    def test_tool_schemas_fit_the_supported_strict_api_subset(self):
        # The raw API rejects numeric/length/maximum-array constraints.
        # Their limits live in descriptions and are enforced by the controller.
        supported = {"type", "description", "properties", "required", "additionalProperties", "items", "enum"}
        def check(schema):
            self.assertTrue(set(schema) <= supported, set(schema) - supported)
            if schema["type"] == "object":
                self.assertIs(schema["additionalProperties"], False)
                for field in schema["properties"].values():
                    check(field)
            elif schema["type"] == "array":
                check(schema["items"])
        for tool in review.TOOLS:
            self.assertTrue(tool["strict"])
            check(tool["input_schema"])

    def test_blocking_findings_need_their_actual_source_anchor_in_each_pass(self):
        sources = review.Sources(FakeGitHub(), PR)
        report = {**REPORT, "findings": [FINDING]}
        with self.assertRaisesRegex(ValueError, "anchor source line"):
            review.validate_report(report, PR["review_files"], sources)
        sources.read("src/lib.rs", "head", 8, 1)
        self.assertEqual(review.validate_report(report, PR["review_files"], sources), report)
        sources.read_ranges.clear()
        with self.assertRaisesRegex(ValueError, "anchor source line"):
            review.validate_report(report, PR["review_files"], sources)

    def test_lockfile_finding_uses_only_actual_lines_in_complete_provided_patch(self):
        file = {"filename": "Cargo.lock", "status": "modified", "additions": 2,
                "deletions": 1, "patch": "@@ -8,2 +8,3 @@\n context\n-old\n+new\n+dependency"}
        sources = review.Sources(FakeGitHub(), PR)
        for line in (8, 9, 10):
            report = {**REPORT, "findings": [{**FINDING, "path": "Cargo.lock", "line": line}]}
            self.assertEqual(review.validate_report(report, [file], sources), report)
        for line in (7, 11, 100):
            with self.assertRaisesRegex(ValueError, "anchor source line"):
                review.validate_report({**REPORT, "findings": [{**FINDING, "path": "Cargo.lock", "line": line}]},
                                       [file], sources)
        with self.assertRaisesRegex(ValueError, "Cargo.lock"):
            sources.read("Cargo.lock", "head", 8, 1)

    def test_removed_lockfile_uses_base_lines_and_multiple_hunks_have_distinct_ranges(self):
        removed = {"filename": "examples/Cargo.lock", "status": "removed", "additions": 0,
                   "deletions": 1, "patch": "@@ -17 +0,0 @@\n-old\n\\ No newline at end of file"}
        report = {**REPORT, "findings": [{**FINDING, "path": removed["filename"], "line": 17}]}
        sources = review.Sources(FakeGitHub(), PR)
        self.assertEqual(review.validate_report(report, [removed], sources), report)
        self.assertFalse(review.lockfile_patch_anchor(removed, "head", 17))
        multiple = {**removed, "status": "modified", "additions": 1, "deletions": 1,
                    "patch": "@@ -2 +2,0 @@\n-old\n@@ -20,0 +20 @@\n+new"}
        self.assertTrue(review.lockfile_patch_anchor(multiple, "base", 2))
        self.assertTrue(review.lockfile_patch_anchor(multiple, "head", 20))
        self.assertFalse(review.lockfile_patch_anchor(multiple, "head", 2))
        self.assertFalse(review.lockfile_patch_anchor(multiple, "base", 20))

    def test_missing_truncated_or_generated_patches_never_bypass_source_evidence(self):
        file = {"filename": "Cargo.lock", "status": "modified", "additions": 1,
                "deletions": 1, "patch": "@@ -8 +8 @@\n-old\n+new"}
        for change in ({"patch": None}, {"additions": 2},
                       {"patch": "@@ -8,2 +8,2 @@\n-old\n+new"},
                       {"filename": "src/lib.rs"},
                       {"filename": "verification/chip/facts/Cargo.lock"},
                       {"filename": "chip/pac/src/generated.rs"}):
            with self.subTest(change=change):
                invalid = {**file, **change}
                report = {**REPORT, "findings": [{**FINDING, "path": invalid["filename"]}]}
                with self.assertRaisesRegex(ValueError, "anchor source line"):
                    review.validate_report(report, [invalid], review.Sources(FakeGitHub(), PR))

    def test_both_passes_can_report_lockfile_defects_without_full_file_reads(self):
        file = {"filename": "Cargo.lock", "status": "modified", "additions": 1,
                "deletions": 1, "patch": "@@ -8 +8 @@\n-old\n+new"}
        report = {**REPORT, "findings": [{**FINDING, "path": "Cargo.lock"}]}
        claude = review.Claude(review.MODEL)
        sources = review.Sources(FakeGitHub(), PR)
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            return {"usage": {"input_tokens": 10, "output_tokens": 1}, "stop_reason": "tool_use",
                    "content": [{"type": "tool_use", "id": "finish", "name": "finish_review", "input": report}]}
        with patch.object(claude, "request", side_effect=request), patch.object(sources, "get") as get:
            self.assertEqual(claude.review("{}", sources, [file]), report)
        get.assert_not_called()

    def test_architectural_report_separates_preexisting_defects_without_blocking(self):
        report = {**REPORT, "out_of_scope_findings": [UNRELATED]}
        text = review.render(PR, report, True, "CI", review.MODEL)
        self.assertIn("Архитектурное ревью PR #7", text)
        self.assertIn(review.MODEL, text)
        self.assertIn("можно мерджить", text)
        self.assertIn("Проблемы вне текущего PR", text)
        self.assertIn("issue автоматически не создаются", text)
        self.assertIn(f"/blob/{PR['merge_base_sha']}/src/old.rs#L2", text)

    def test_unrelated_findings_need_base_and_head_lines_read_in_this_pass(self):
        sources = review.Sources(FakeGitHub(), PR)
        report = {**REPORT, "out_of_scope_findings": [UNRELATED]}
        for revision in ("head", "base"):
            with self.assertRaisesRegex(ValueError, "both base and head"):
                review.validate_report(report, PR["review_files"], sources)
            sources.read("src/old.rs", revision, 1, 3)
        self.assertEqual(review.validate_report(report, PR["review_files"], sources), report)
        for change in ({"reason": ""}, {"path": "../.env"}, {"base_line": 4}):
            with self.assertRaises(ValueError):
                review.validate_report({**REPORT, "out_of_scope_findings": [{**UNRELATED, **change}]},
                                       PR["review_files"], sources)
        with self.assertRaises(ValueError):
            review.validate_report({**REPORT, "out_of_scope_findings": [UNRELATED] * 6},
                                   PR["review_files"], sources)

    def test_before_snapshot_uses_merge_base_when_main_has_moved(self):
        api = FakeGitHub()
        text, _, _ = review.context(api, api.pr)
        self.assertEqual(json.loads(text)["diff_base"], PR["merge_base_sha"])
        with patch.object(api, "request", wraps=api.request) as request:
            review.Sources(api, api.pr).read("src/lib.rs", "base", 1, 1)
        self.assertIn(f"ref={PR['merge_base_sha']}", request.call_args.args[1])
        self.assertNotIn(f"ref={PR['base']['sha']}", request.call_args.args[1])

    def test_literal_search_is_bounded_cached_and_obeys_source_restrictions(self):
        api = FakeGitHub()
        data = {"type": "file", "encoding": "base64", "content":
                base64.b64encode(("value.foo\n" * 101).encode()).decode()}
        sources = review.Sources(api, PR)
        with patch.object(api, "request", return_value=data) as request:
            result = json.loads(sources.search("src/lib.rs", "head", "."))
            self.assertEqual(result["total_matches"], 101)
            self.assertEqual(len(result["matches"]), 100)
            self.assertTrue(result["truncated"])
            self.assertEqual(json.loads(sources.search("src/lib.rs", "head", ".*"))["total_matches"], 0)
            self.assertEqual(request.call_count, 1)
        for path in ("../.env", "Cargo.lock", "crates/hardware/chip/pac/raw/src/lib.rs"):
            with self.assertRaises(ValueError):
                sources.search(path, "head", "value")
        with self.assertRaises(ValueError):
            sources.search("src/lib.rs", "head", "")

    def test_verifier_gets_fresh_context_and_can_reject_a_clean_candidate(self):
        claude = review.Claude(review.MODEL)
        final = {**REPORT, "findings": [FINDING]}
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            bodies.append(deepcopy(body))
            if len(bodies) == 2:
                content = [{"type": "tool_use", "id": "read", "name": "read_file",
                            "input": {"path": "src/lib.rs", "revision": "head", "start": 1, "count": 10}}]
            else:
                content = [{"type": "tool_use", "id": "finish", "name": "finish_review",
                            "input": REPORT if len(bodies) == 1 else final}]
            return {"usage": {"input_tokens": 10, "output_tokens": 5}, "stop_reason": "tool_use",
                    "content": content}
        with patch.object(claude, "request", side_effect=request):
            result = claude.review("{}", review.Sources(FakeGitHub(), PR), PR["review_files"])
        self.assertEqual(result, final)
        self.assertEqual(len(bodies), 3)
        self.assertTrue(all(len(body["messages"]) == 1 for body in bodies[:2]))
        self.assertIn("candidate_report", bodies[1]["messages"][0]["content"])
        self.assertIn(review.VERIFY_PROMPT, bodies[1]["system"])
        self.assertTrue(all(body["cache_control"] == {"type": "ephemeral"} for body in bodies))
        self.assertTrue(all(body["output_config"] == {"effort": "high"} for body in bodies))
        self.assertTrue(all(tool["strict"] for tool in review.TOOLS))

    def test_both_passes_share_budget_including_cache_reads_and_writes(self):
        claude = review.Claude(review.MODEL)
        calls = []
        def request(path, body):
            calls.append(path)
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            return {"usage": {"input_tokens": 1, "cache_creation_input_tokens": 1,
                              "cache_read_input_tokens": review.MAX_INPUT - 2, "output_tokens": 1},
                    "stop_reason": "tool_use", "content": [{"type": "tool_use", "id": "finish",
                        "name": "finish_review", "input": REPORT}]}
        with patch.object(claude, "request", side_effect=request):
            with self.assertRaisesRegex(ValueError, "лимит токенов"):
                claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        self.assertEqual(claude.input_used, review.MAX_INPUT)
        self.assertEqual(calls, ["messages/count_tokens", "messages", "messages/count_tokens"])

    def test_actual_usage_above_preflight_estimate_cannot_finish_green(self):
        claude = review.Claude(review.MODEL)
        response = {"usage": {"input_tokens": 1, "cache_read_input_tokens": review.MAX_INPUT,
                              "output_tokens": 1}, "stop_reason": "tool_use",
                    "content": [{"type": "tool_use", "id": "finish", "name": "finish_review", "input": REPORT}]}
        with patch.object(claude, "request", side_effect=[{"input_tokens": 10}, response]) as request:
            with self.assertRaisesRegex(ValueError, "лимит токенов"):
                claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        self.assertEqual(request.call_count, 2)

    def test_verifier_must_read_its_own_evidence_after_rejected_unrelated_anchor(self):
        claude = review.Claude(review.MODEL)
        source_reads = [{"type": "tool_use", "id": rev, "name": "read_file",
                         "input": {"path": "src/old.rs", "revision": rev, "start": 1, "count": 3}}
                        for rev in ("head", "base")]
        finish = [{"type": "tool_use", "id": "finish", "name": "finish_review",
                   "input": {**REPORT, "out_of_scope_findings": [UNRELATED]}}]
        responses = iter([source_reads, finish, finish, source_reads, finish])
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            bodies.append(deepcopy(body))
            return {"usage": {"input_tokens": 10, "output_tokens": 10}, "stop_reason": "tool_use",
                    "content": next(responses)}
        with patch.object(claude, "request", side_effect=request):
            result = claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        self.assertEqual(result["out_of_scope_findings"], [UNRELATED])
        rejected = bodies[3]["messages"][-1]["content"][0]
        self.assertTrue(rejected["is_error"])
        self.assertIn("both base and head", rejected["content"])

    def test_invalid_finish_can_be_repaired_after_reading_its_anchor(self):
        claude = review.Claude(review.MODEL)
        report = {**REPORT, "findings": [FINDING]}
        finish = [{"type": "tool_use", "id": "finish", "name": "finish_review", "input": report}]
        read = [{"type": "tool_use", "id": "read", "name": "read_file",
                 "input": {"path": "src/lib.rs", "revision": "head", "start": 8, "count": 1}}]
        responses = iter([finish, read, finish, read, finish])
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            bodies.append(deepcopy(body))
            return {"usage": {"input_tokens": 10, "output_tokens": 1}, "stop_reason": "tool_use",
                    "content": next(responses)}
        with patch.object(claude, "request", side_effect=request):
            self.assertEqual(claude.review("{}", review.Sources(FakeGitHub(), PR), PR["review_files"]), report)
        rejected = bodies[1]["messages"][-1]["content"][0]
        self.assertTrue(rejected["is_error"])
        self.assertIn("anchor source line", rejected["content"])

    def test_repeated_invalid_reports_exhaust_budget_and_cannot_publish_green(self):
        api = FakeGitHub()
        api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                     "conclusion": "success", "html_url": "https://example/ci"}]
        reports = []
        def request(self, path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            reports.append(deepcopy(body))
            return {"usage": {"input_tokens": 10, "output_tokens": 1}, "stop_reason": "tool_use",
                    "content": [{"type": "tool_use", "id": "finish", "name": "finish_review",
                                 "input": {**REPORT, "findings": [FINDING]}}]}
        with patch.object(review.Claude, "request", new=request):
            with self.assertRaisesRegex(ValueError, "лимит шагов"):
                review.review_pr(api, 7, review.MODEL)
        self.assertEqual(len(reports), 20)
        self.assertTrue(reports[-1]["messages"][-1]["content"][0]["is_error"])
        self.assertEqual(api.writes[-1][2]["state"], "error")
        self.assertNotIn("success", [w[2]["state"] for w in api.writes if w[1].startswith("statuses/")])

    def test_finish_batched_with_source_calls_requires_a_separate_retry(self):
        claude = review.Claude(review.MODEL)
        finish = {"type": "tool_use", "id": "finish", "name": "finish_review", "input": REPORT}
        read = {"type": "tool_use", "id": "read", "name": "read_file",
                "input": {"path": "src/lib.rs", "revision": "head", "start": 8, "count": 1}}
        responses = iter([[finish, read], [finish], [finish]])
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            bodies.append(deepcopy(body))
            return {"usage": {"input_tokens": 10, "output_tokens": 1}, "stop_reason": "tool_use",
                    "content": next(responses)}
        with patch.object(claude, "request", side_effect=request):
            self.assertEqual(claude.review("{}", review.Sources(FakeGitHub(), PR), []), REPORT)
        results = bodies[1]["messages"][-1]["content"]
        self.assertEqual(len(results), 2)
        self.assertTrue(results[0]["is_error"])
        self.assertIn("only tool call", results[0]["content"])
        self.assertNotIn("is_error", results[1])
        self.assertEqual(len(bodies), 3)

    def test_verifier_failure_never_publishes_the_analysts_clean_verdict(self):
        api = FakeGitHub()
        api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                     "conclusion": "success", "html_url": "https://example/ci"}]
        with patch.object(review.Claude, "review_pass", side_effect=[REPORT, ValueError("Verification failed")]):
            with self.assertRaisesRegex(ValueError, "Verification failed"):
                review.review_pr(api, 7, review.MODEL)
        self.assertEqual(api.writes[-1][2]["state"], "error")
        self.assertNotIn("success", [w[2]["state"] for w in api.writes if w[1].startswith("statuses/")])

    def test_lockfile_changes_remain_in_diff_without_full_lockfile_reads(self):
        api = FakeGitHub()
        api.files[0].update(filename="Cargo.lock", additions=1, deletions=0,
                            patch='@@ -2,0 +3 @@\n+ "oer-time",')
        text, _, gaps = review.context(api, api.pr)
        self.assertIn('oer-time', text)
        self.assertEqual(gaps, [])
        sources = review.Sources(api, api.pr)
        for path in ('Cargo.lock', 'examples/esp32s31/Cargo.lock'):
            with self.assertRaisesRegex(ValueError, 'Cargo.lock'):
                sources.read(path, 'head')

    def test_old_prs_always_run_the_controller_from_the_default_branch(self):
        workflow = (Path(__file__).resolve().parents[1] / 'workflows/claude-review.yml').read_text()
        refs = re.findall(r'^\s+ref: (.+)$', workflow, re.MULTILINE)
        self.assertTrue(refs)
        self.assertEqual(set(refs), {'${{ github.event.repository.default_branch }}'})
        self.assertIn("github.event.pull_request.state == 'open'", workflow)

    def test_full_context_of_a_103_file_pr_is_not_truncated(self):
        api = FakeGitHub()
        api.files = [{"filename": f"src/component_{i}.rs", "status": "added", "additions": 1,
                      "deletions": 0, "patch": "@@ -0,0 +1 @@\n+" + "x" * 2000}
                     for i in range(103)]
        api.pr["changed_files"] = 103
        text, files, gaps = review.context(api, api.pr)
        data = json.loads(text)
        self.assertGreater(len(text), 180_000)
        self.assertEqual(len(files), 103)
        self.assertEqual(len(data["changes"]), 103)
        self.assertTrue(all(c["patch"].endswith("x" * 2000) for c in data["changes"]))
        self.assertEqual(gaps, [])
        with self.assertRaisesRegex(ValueError, "превышает лимит"):
            review.limited({"large": "x" * review.MAX_CONTEXT})

    def test_auth_probe_calls_messages_without_publishing_or_printing_tokens(self):
        with patch.object(review, "Claude") as claude, patch("sys.stdout", new_callable=io.StringIO) as output:
            claude.return_value.request.return_value = {"content": [{"type": "text", "text": "OK"}]}
            review.verify_auth(review.MODEL)
            args = claude.return_value.request.call_args.args
            self.assertEqual(args[0], "messages")
            self.assertEqual(args[1]["max_tokens"], 16)
            self.assertIn("OIDC exchange and Messages API succeeded", output.getvalue())
            claude.return_value.request.return_value = {"content": []}
            with self.assertRaises(ValueError):
                review.verify_auth(review.MODEL)

    def test_approval_requires_complete_analysis_and_successful_ci(self):
        for ci in (None, False):
            self.assertIn("Проверка неполная", review.render(PR, REPORT, ci, "CI pending", review.MODEL))
        incomplete = {**REPORT, "coverage_gaps": ["Missing issue acceptance criteria"]}
        self.assertIn("Проверка неполная", review.render(PR, incomplete, True, "CI", review.MODEL))
        self.assertIn("можно мерджить", review.render(PR, REPORT, True, "CI", review.MODEL))
        failed = {**REPORT, "findings": [FINDING]}
        text = review.render(PR, failed, True, "CI", review.MODEL)
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

    def test_every_report_appends_a_new_comment_without_editing_existing_reports(self):
        api = FakeGitHub()
        api.comments = [{"id": 1, "user": {"login": "attacker"}, "body": review.MARKER},
                        {"id": 2, "user": {"login": "github-actions[bot]"},
                         "body": review.MARKER + "\nPrevious finding"}]
        previous = deepcopy(api.comments)
        with patch.object(api, "list", wraps=api.list) as listing:
            self.assertTrue(review.publish(api, PR, "First report"))
            self.assertTrue(review.publish(api, PR, "Second report"))
        listing.assert_not_called()
        self.assertEqual(api.comments, previous)
        self.assertEqual([w[:2] for w in api.writes], [("POST", "issues/7/comments")] * 2)
        self.assertIn("First report", api.writes[0][2]["body"])
        self.assertIn("Second report", api.writes[1][2]["body"])

    def test_report_identifies_its_commit_and_exact_workflow_attempt(self):
        api = FakeGitHub()
        with patch.dict(os.environ, {"GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2"}):
            review.publish(api, PR, "Report")
        body = api.writes[0][2]["body"]
        self.assertIn(f"Head: `{PR['head']['sha']}`", body)
        self.assertIn(f"Base: `{PR['base']['sha']}`", body)
        self.assertIn(f"https://github.com/{review.REPOSITORY}/actions/runs/123/attempts/2", body)

    def test_local_cross_repository_and_closing_issue_links_are_read(self):
        refs = review.linked_issues("Fixes #12, ermacv/another#5 and https://github.com/x/y/issues/9")
        self.assertEqual(refs, {(review.REPOSITORY, 12), ("ermacv/another", 5), ("x/y", 9)})
        text, _, gaps = review.context(FakeGitHub(), PR)
        self.assertIn('"number": 12', text)
        self.assertIn('"number": 13', text)
        self.assertEqual(gaps, [])
        issues = {issue["number"]: issue for issue in json.loads(text)["linked_issues"]}
        self.assertEqual(issues[13]["relationship"], "closing")
        self.assertEqual(issues[12]["relationship"], "closing")
        self.assertEqual(issues[14]["relationship"], "reference")

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
                review.validate_report({**REPORT, "findings": [bad]}, PR["review_files"],
                                       review.Sources(FakeGitHub(), PR))

    def test_ci_event_does_not_review_stale_or_other_repository_revisions(self):
        event = {"workflow_run": {"head_repository": {"full_name": review.REPOSITORY},
                                  "head_sha": PR["head"]["sha"]}}
        self.assertEqual(review.targets(FakeGitHub(), event, "workflow_run"), [7])
        event["workflow_run"]["head_repository"]["full_name"] = "x/y"
        self.assertEqual(review.targets(FakeGitHub(), event, "workflow_run"), [])

    def test_waiting_for_ci_sets_pending_without_comments_or_spending_api_credits(self):
        api = FakeGitHub()
        with patch.object(review, "Claude") as claude:
            review.review_pr(api, 7, review.MODEL)
        claude.assert_not_called()
        comments = [w for w in api.writes if w[1] == "issues/7/comments"]
        self.assertEqual(comments, [])
        self.assertEqual([w[2]["state"] for w in api.writes if w[1].startswith("statuses/")], ["pending"])

    def test_comment_failure_cannot_leave_an_old_success_status_effective(self):
        api = FakeGitHub()
        api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                     "conclusion": "success", "html_url": "https://example/ci"}]
        review.status(api, PR, "success", "Previous approval")
        with patch.object(review.Claude, "review", return_value=deepcopy(REPORT)), \
                patch.object(review, "publish", side_effect=RuntimeError("Comment unavailable")):
            with self.assertRaises(RuntimeError):
                review.review_pr(api, 7, review.MODEL)
        states = [w[2]["state"] for w in api.writes if w[1].startswith("statuses/")]
        self.assertEqual(states, ["success", "pending"])

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

    def test_api_failure_appends_incomplete_report_and_preserves_previous_findings(self):
        api = FakeGitHub()
        api.comments = [{"id": 2, "user": {"login": "github-actions[bot]"},
                         "body": review.MARKER + "\nPrevious finding"}]
        previous = deepcopy(api.comments)
        api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                     "conclusion": "success", "html_url": "https://example/ci"}]
        with patch.object(review, "Claude", side_effect=ValueError("Federation not configured")):
            with self.assertRaises(ValueError):
                review.review_pr(api, 7, review.MODEL)
        comments = [w for w in api.writes if w[1] == "issues/7/comments"]
        self.assertEqual(len(comments), 1)
        self.assertIn("Проверка неполная", comments[-1][2]["body"])
        self.assertEqual(api.comments, previous)
        self.assertTrue(all(w[0] == "POST" for w in api.writes))
        self.assertEqual(api.writes[-1][2]["state"], "error")

    def test_commit_status_only_succeeds_for_complete_clean_analysis_and_ci(self):
        reports = [REPORT, {**REPORT, "findings": [FINDING]},
                   {**REPORT, "coverage_gaps": ["Unreadable source"]},
                   {**REPORT, "out_of_scope_findings": [UNRELATED]}]
        for report, expected in zip(reports, ["success", "failure", "failure", "success"]):
            api = FakeGitHub()
            api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                         "conclusion": "success", "html_url": "https://example/ci"}]
            with patch.object(review, "Claude") as claude:
                claude.return_value.review.return_value = deepcopy(report)
                review.review_pr(api, 7, review.MODEL)
            self.assertEqual(api.writes[-1][1], f"statuses/{PR['head']['sha']}")
            self.assertEqual(api.writes[-1][2]["state"], expected)
            self.assertEqual(len([w for w in api.writes if w[1] == "issues/7/comments"]), 1)

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
