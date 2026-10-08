"""Offline regressions for verdicts, immutable reads and event races."""

import base64
import io
import json
import os
from pathlib import Path
import re
import tempfile
from copy import deepcopy
from decimal import Decimal
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


def report_blocks(report):
    return [{"type": "text", "text": json.dumps(report)}]


def message(blocks, usage=None):
    return {"usage": usage or {"input_tokens": 10, "output_tokens": 1},
            "stop_reason": "tool_use" if any(b["type"] == "tool_use" for b in blocks) else "end_turn",
            "content": blocks}


class FakeGitHub:
    def __init__(self):
        self.pr = deepcopy(PR)
        self.comments = []
        self.writes = []
        self.runs = []
        self.status_records = []
        self.issue_text = "Wake correctly"
        self.files = [{"filename": "src/lib.rs", "status": "modified", "additions": 1,
                       "deletions": 1, "patch": "@@ -8 +8 @@\n-old\n+new"}]

    def request(self, method, path, data=None):
        if method != "GET":
            self.writes.append((method, path, deepcopy(data)))
            if path.startswith("statuses/"):
                self.status_records.insert(0, {**deepcopy(data),
                    "creator": {"login": "github-actions[bot]", "type": "Bot"}})
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
        if path.endswith("/statuses"):
            return deepcopy(self.status_records)
        if path.startswith("commits/"):
            return [deepcopy(self.pr), {**deepcopy(self.pr), "number": 8,
                                       "head": {"sha": "c" * 40}}]
        raise AssertionError(path)

    def closing_issues(self, number):
        return {(review.REPOSITORY, 12), (review.REPOSITORY, 13)}

    def read_issue(self, repo, number):
        return {"repository": repo, "number": number, "title": self.issue_text, "comments": []}


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
        check(review.REPORT_SCHEMA)

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
            return message(report_blocks(report))
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
                content = report_blocks(REPORT if len(bodies) == 1 else final)
            return message(content, {"input_tokens": 10, "output_tokens": 5})
        with patch.object(claude, "request", side_effect=request):
            result = claude.review("{}", review.Sources(FakeGitHub(), PR), PR["review_files"])
        self.assertEqual(result, final)
        self.assertEqual(len(bodies), 3)
        self.assertTrue(all(len(body["messages"]) == 1 for body in bodies[:2]))
        first, verifier = [body["messages"][0]["content"] for body in bodies[:2]]
        self.assertEqual(first[0], verifier[0])
        self.assertEqual(first[0], {"type": "text", "text": "{}", "cache_control": {"type": "ephemeral"}})
        self.assertIn("candidate_report", verifier[1]["text"])
        self.assertIn(review.VERIFY_PROMPT, verifier[1]["text"])
        self.assertTrue(all(body["system"] == review.PROMPT for body in bodies))
        self.assertEqual(bodies[0]["tools"], bodies[1]["tools"])
        self.assertTrue(all(body["cache_control"] == {"type": "ephemeral"} for body in bodies))
        self.assertTrue(all(body["output_config"] == {"effort": "high", "format": {
            "type": "json_schema", "schema": review.REPORT_SCHEMA}} for body in bodies))
        self.assertTrue(all(body.get("tool_choice", {"type": "auto"}) == {"type": "auto"} for body in bodies))
        self.assertTrue(all(tool["strict"] for tool in review.TOOLS))

    def test_cached_investigation_above_two_million_tokens_can_reach_verification(self):
        claude = review.Claude(review.MODEL)
        responses = []
        # Reproduce the large-PR usage that exhausted the old cumulative-input
        # cap after eight source turns, despite spending only about $1.72.
        writes = [225883, 8917, 2671, 1165, 318, 2141, 775, 4711]
        reads = [0, 225883, 234800, 237471, 238636, 238954, 241095, 241870]
        outputs = [5804, 411, 448, 128, 147, 429, 216, 259]
        for write, read, output in zip(writes, reads, outputs):
            responses.append(message([{"type": "tool_use", "id": str(write), "name": "read_file",
                "input": {"path": "src/lib.rs", "revision": "head", "start": 1, "count": 10}}],
                {"input_tokens": 4 if not read else 2, "cache_creation_input_tokens": write,
                 "cache_read_input_tokens": read, "output_tokens": output}))
        for read in (246581, 225883):
            responses.append(message(report_blocks(REPORT), {"input_tokens": 20,
                "cache_creation_input_tokens": 0, "cache_read_input_tokens": read, "output_tokens": 1000}))
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 250000}
            bodies.append(deepcopy(body))
            return responses.pop(0)
        with patch.object(claude, "request", side_effect=request):
            result = claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        self.assertEqual(result, REPORT)
        self.assertEqual(len(bodies), 10)
        self.assertGreater(claude.input_used, 2_000_000)
        self.assertLess(claude.spent_usd(), Decimal("2"))
        self.assertEqual(len(bodies[-1]["messages"]), 1)
        self.assertEqual(claude.usage["verification"]["cache_read_input_tokens"], 225883)

    def test_each_pass_can_investigate_beyond_twenty_calls_within_five_dollars(self):
        claude = review.Claude(review.MODEL)
        bodies = []
        counts = []
        def request(path, body):
            if path == "messages/count_tokens":
                counts.append(deepcopy(body))
                return {"input_tokens": 250000}
            bodies.append(deepcopy(body))
            step = (len(bodies) - 1) % 22 + 1
            content = report_blocks(REPORT) if step == 22 else [
                {"type": "tool_use", "id": f"read-{step}", "name": "read_file",
                 "input": {"path": "src/lib.rs", "revision": "head", "start": 1, "count": 10}}]
            usage = {"input_tokens": 2, "cache_read_input_tokens": 225883, "output_tokens": 200}
            if len(bodies) == 1:
                usage = {"input_tokens": 4, "cache_creation_input_tokens": 225883, "output_tokens": 200}
            return message(content, usage)
        with patch.object(claude, "request", side_effect=request):
            self.assertEqual(claude.review("{}", review.Sources(FakeGitHub(), PR), []), REPORT)
        self.assertEqual(len(bodies), 44)
        self.assertLess(claude.spent_usd(), Decimal("5"))
        self.assertEqual(len(bodies[22]["messages"]), 1)
        self.assertEqual(bodies[22]["messages"][0]["content"][0], bodies[0]["messages"][0]["content"][0])
        self.assertTrue(all(a["messages"] == b["messages"] for a, b in zip(counts, bodies)))

    def test_progress_preserves_history_and_updates_shared_budget_in_both_passes(self):
        claude = review.Claude(review.MODEL)
        bodies = []
        signed = {"type": "thinking", "thinking": "PRIVATE_THINKING", "signature": "SIGNED"}
        read = {"type": "tool_use", "id": "read", "name": "read_file",
                "input": {"path": "src/lib.rs", "revision": "head", "start": 1, "count": 10}}
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            bodies.append(deepcopy(body))
            return message([signed, read] if len(bodies) in (1, 3) else report_blocks(REPORT),
                           {"input_tokens": 10, "output_tokens": 5})
        with patch.object(claude, "request", side_effect=request):
            claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        initial = bodies[0]["messages"][0]["content"][-1]["text"]
        second = bodies[1]["messages"][-1]["content"][-1]["text"]
        verifier = bodies[2]["messages"][0]["content"][-1]["text"]
        self.assertIn("phase=analysis; request 1/", initial)
        self.assertIn("$0.000000/$5.00", initial)
        self.assertIn("request 2/", second)
        self.assertIn("$0.000140/$5.00", second)
        self.assertIn(f"{review.MAX_OUTPUT - 5} output tokens remain", second)
        self.assertIn("phase=verification; request 1/", verifier)
        self.assertIn("$0.000280/$5.00", verifier)
        self.assertEqual(bodies[1]["messages"][:1], bodies[0]["messages"])
        self.assertEqual(bodies[1]["messages"][1]["content"], [signed, read])
        self.assertEqual(bodies[1]["messages"][-1]["content"][0]["type"], "tool_result")
        self.assertEqual(len(bodies[2]["messages"]), 1)

    def test_tool_diagnostics_count_batches_and_errors_without_logging_arguments(self):
        claude = review.Claude(review.MODEL)
        valid = {"type": "tool_use", "id": "good", "name": "read_file",
                 "input": {"path": "src/lib.rs", "revision": "head", "start": 1, "count": 10}}
        invalid = {**deepcopy(valid), "id": "bad"}
        invalid["input"]["path"] = "../PRIVATE_PATH"
        responses = iter((message([valid, invalid]), message(report_blocks(REPORT)), message(report_blocks(REPORT))))
        def request(path, body):
            return {"input_tokens": 10} if path == "messages/count_tokens" else next(responses)
        with patch.object(claude, "request", side_effect=request), \
                patch("sys.stdout", new_callable=io.StringIO) as output:
            claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        records = [json.loads(line) for line in output.getvalue().splitlines() if line.startswith("{")]
        tools = next(r for r in records if r["event"] == "claude_tools")
        self.assertEqual(tools, {"event": "claude_tools", "phase": "analysis", "step": 1, "calls": 2, "errors": 1})
        response = next(r for r in records if r["event"] == "claude_response")
        self.assertEqual(response["tool_calls"], 2)
        self.assertNotIn("PRIVATE_PATH", output.getvalue())

    def test_both_passes_share_dollar_budget(self):
        claude = review.Claude(review.MODEL)
        calls = []
        def request(path, body):
            calls.append(path)
            if path == "messages/count_tokens":
                return {"input_tokens": 100000}
            return message(report_blocks(REPORT), {"input_tokens": 0, "cache_creation_input_tokens": 100000,
                                                   "output_tokens": 1})
        with patch.object(review, "MAX_COST_USD", Decimal("0.60")), \
                patch.object(claude, "request", side_effect=request):
            with self.assertRaisesRegex(ValueError, "Недостаточно бюджета"):
                claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        self.assertEqual(claude.spent_usd(), Decimal("0.500020"))
        self.assertEqual(calls, ["messages/count_tokens", "messages", "messages/count_tokens"])

    def test_exhausted_budget_cannot_publish_a_clean_candidate_or_completion_receipt(self):
        api = self.successful_api()
        calls = []
        def request(self, path, body):
            calls.append(path)
            if path == "messages/count_tokens":
                return {"input_tokens": 100000}
            return message(report_blocks({**REPORT, "summary": "CANDIDATE_NOT_FINAL"}),
                           {"input_tokens": 0, "cache_creation_input_tokens": 100000, "output_tokens": 1})
        with patch.object(review, "MAX_COST_USD", Decimal("0.60")), \
                patch.object(review.Claude, "request", new=request):
            with self.assertRaisesRegex(ValueError, "Недостаточно бюджета"):
                review.review_pr(api, 7, review.MODEL)
        self.assertEqual(calls, ["messages/count_tokens", "messages", "messages/count_tokens"])
        comments = [w[2]["body"] for w in api.writes if w[1] == "issues/7/comments"]
        self.assertEqual(len(comments), 1)
        self.assertIn("Проверка неполная", comments[0])
        self.assertIn("0.500020", comments[0])
        self.assertNotIn("CANDIDATE_NOT_FINAL", comments[0])
        self.assertEqual([r["state"] for r in api.status_records], ["error", "pending"])
        self.assertFalse(any(r["description"].startswith(review.COMPLETED) for r in api.status_records))

    def test_actual_usage_above_preflight_estimate_cannot_finish_green(self):
        claude = review.Claude(review.MODEL)
        response = message(report_blocks(REPORT), {"input_tokens": 1, "cache_creation_input_tokens": 3000,
                                                  "output_tokens": 1})
        with patch.object(review, "MAX_COST_USD", Decimal("0.01")), \
                patch.object(claude, "request", side_effect=[{"input_tokens": 10}, response]) as request:
            with self.assertRaisesRegex(ValueError, "превысил бюджет"):
                claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        self.assertEqual(request.call_count, 2)

    def test_preflight_does_not_assume_the_next_request_will_hit_cache(self):
        claude = review.Claude(review.MODEL)
        estimates = iter((100000, 200000))
        calls = []
        def request(path, body):
            calls.append(path)
            if path == "messages/count_tokens":
                return {"input_tokens": next(estimates)}
            return message(report_blocks(REPORT), {"input_tokens": 0, "cache_read_input_tokens": 100000,
                                                   "output_tokens": 1})
        with patch.object(review, "MAX_COST_USD", Decimal("1")), \
                patch.object(claude, "request", side_effect=request):
            with self.assertRaisesRegex(ValueError, "без попаданий в кеш"):
                claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        self.assertEqual(calls, ["messages/count_tokens", "messages", "messages/count_tokens"])

    def test_remaining_dollars_cap_output_after_reserving_a_full_cache_miss(self):
        claude = review.Claude(review.MODEL)
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 1000}
            bodies.append(deepcopy(body))
            return message(report_blocks(REPORT), {"input_tokens": 10, "output_tokens": 5})
        with patch.object(review, "MAX_COST_USD", Decimal("0.01")), \
                patch.object(claude, "request", side_effect=request):
            self.assertEqual(claude.review("{}", review.Sources(FakeGitHub(), PR), []), REPORT)
        self.assertEqual([b["max_tokens"] for b in bodies], [250, 243])
        self.assertEqual(claude.spent_usd(), Decimal("0.000280"))

    def test_verifier_must_read_its_own_evidence_after_rejected_unrelated_anchor(self):
        claude = review.Claude(review.MODEL)
        source_reads = [{"type": "tool_use", "id": rev, "name": "read_file",
                         "input": {"path": "src/old.rs", "revision": rev, "start": 1, "count": 3}}
                        for rev in ("head", "base")]
        finish = report_blocks({**REPORT, "out_of_scope_findings": [UNRELATED]})
        responses = iter([source_reads, finish, finish, source_reads, finish])
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            bodies.append(deepcopy(body))
            return message(next(responses), {"input_tokens": 10, "output_tokens": 10})
        with patch.object(claude, "request", side_effect=request):
            result = claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        self.assertEqual(result["out_of_scope_findings"], [UNRELATED])
        rejected = bodies[3]["messages"][-1]["content"]
        self.assertIn("Report rejected", rejected)
        self.assertIn("both base and head", rejected)

    def test_invalid_report_can_be_repaired_after_reading_its_anchor(self):
        claude = review.Claude(review.MODEL)
        report = {**REPORT, "findings": [FINDING]}
        finish = report_blocks(report)
        read = [{"type": "tool_use", "id": "read", "name": "read_file",
                 "input": {"path": "src/lib.rs", "revision": "head", "start": 8, "count": 1}}]
        responses = iter([finish, read, finish, read, finish])
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            bodies.append(deepcopy(body))
            return message(next(responses))
        with patch.object(claude, "request", side_effect=request):
            self.assertEqual(claude.review("{}", review.Sources(FakeGitHub(), PR), PR["review_files"]), report)
        rejected = bodies[1]["messages"][-1]["content"]
        self.assertIn("Report rejected", rejected)
        self.assertIn("anchor source line", rejected)

    def test_repeated_invalid_reports_exhaust_budget_and_cannot_publish_green(self):
        api = FakeGitHub()
        api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                     "conclusion": "success", "html_url": "https://example/ci"}]
        reports = []
        def request(self, path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            reports.append(deepcopy(body))
            return message(report_blocks({**REPORT, "findings": [FINDING]}))
        with patch.object(review.Claude, "request", new=request):
            with self.assertRaisesRegex(ValueError, "лимит шагов"):
                review.review_pr(api, 7, review.MODEL)
        self.assertEqual(len(reports), review.MAX_STEPS)
        self.assertIn("Report rejected", reports[-1]["messages"][-1]["content"])
        self.assertEqual(api.writes[-1][2]["state"], "error")
        self.assertNotIn("success", [w[2]["state"] for w in api.writes if w[1].startswith("statuses/")])

    def test_json_alongside_tool_calls_cannot_approve_before_tools_are_resolved(self):
        claude = review.Claude(review.MODEL)
        finish = report_blocks(REPORT)[0]
        read = {"type": "tool_use", "id": "read", "name": "read_file",
                "input": {"path": "src/lib.rs", "revision": "head", "start": 8, "count": 1}}
        responses = iter([[finish, read], [finish], [finish]])
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            bodies.append(deepcopy(body))
            return message(next(responses))
        with patch.object(claude, "request", side_effect=request):
            self.assertEqual(claude.review("{}", review.Sources(FakeGitHub(), PR), []), REPORT)
        results = [b for b in bodies[1]["messages"][-1]["content"] if b["type"] == "tool_result"]
        self.assertEqual(len(results), 1)
        self.assertEqual(results[0]["tool_use_id"], "read")
        self.assertNotIn("is_error", results[0])
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

    def test_structured_end_turn_with_thinking_finishes_both_passes_and_counts_the_schema(self):
        for model in review.PRICES:
            with self.subTest(model=model):
                claude = review.Claude(model)
                requests = []
                def request(path, body):
                    requests.append((path, deepcopy(body)))
                    if path == "messages/count_tokens":
                        return {"input_tokens": 10}
                    return message([{"type": "thinking", "thinking": "PRIVATE_REASONING", "signature": "SIGNATURE"},
                                    *report_blocks(REPORT)])
                with patch.object(claude, "request", side_effect=request), \
                        patch("sys.stdout", new_callable=io.StringIO) as output:
                    self.assertEqual(claude.review("{}", review.Sources(FakeGitHub(), PR), []), REPORT)
                self.assertEqual(len(requests), 4)
                for count, paid in (requests[:2], requests[2:]):
                    self.assertEqual(count[1]["output_config"], paid[1]["output_config"])
                    self.assertEqual(paid[1]["output_config"]["format"]["schema"], review.REPORT_SCHEMA)
                    self.assertEqual(paid[1]["max_tokens"], 24000)
                    self.assertEqual({t["name"] for t in paid[1]["tools"]},
                                     {"read_file", "search_file", "list_directory"})
                self.assertNotIn("PRIVATE_REASONING", output.getvalue())
                self.assertNotIn("SIGNATURE", output.getvalue())

    def test_truncated_refused_or_unknown_response_never_uses_a_clean_looking_json_report(self):
        for reason in ("max_tokens", "model_context_window_exceeded", "refusal", "pause_turn", "stop_sequence",
                       "PRIVATE_UNKNOWN_REASON"):
            with self.subTest(reason=reason):
                api = self.successful_api()
                response = {**message(report_blocks(REPORT)), "stop_reason": reason}
                with patch.object(review.Claude, "request", side_effect=[{"input_tokens": 10}, response]) as request, \
                        patch("sys.stdout", new_callable=io.StringIO) as output:
                    with self.assertRaisesRegex(ValueError, "stop_reason="):
                        review.review_pr(api, 7, review.MODEL)
                self.assertEqual(request.call_count, 2)
                self.assertEqual(api.status_records[0]["state"], "error")
                self.assertFalse(any(r["state"] == "success" for r in api.status_records))
                self.assertFalse(any(r["description"].startswith(review.COMPLETED) for r in api.status_records))
                body = next(w[2]["body"] for w in api.writes if w[1] == "issues/7/comments")
                self.assertIn("stop_reason=" + ("unknown" if reason.startswith("PRIVATE") else reason), body)
                self.assertNotIn("PRIVATE_UNKNOWN_REASON", output.getvalue() + body)

    def test_malformed_empty_and_contradictory_final_responses_are_not_accepted(self):
        for response in (message([{"type": "text", "text": "PRIVATE_NOT_JSON"}]), message([]),
                         message([*report_blocks(REPORT), *report_blocks(REPORT)]),
                         {**message([{"type": "tool_use", "id": "bad", "name": "read_file", "input": {}}]),
                          "stop_reason": "end_turn"},
                         {**message(report_blocks(REPORT)), "stop_reason": "tool_use"}):
            with self.subTest(response=response):
                claude = review.Claude(review.MODEL)
                with patch.object(claude, "request", side_effect=[{"input_tokens": 10}, response]) as request, \
                        patch("sys.stdout", new_callable=io.StringIO) as output:
                    with self.assertRaises(ValueError) as error:
                        claude.review("{}", review.Sources(FakeGitHub(), PR), [])
                self.assertEqual(request.call_count, 2)
                self.assertNotIn("PRIVATE_NOT_JSON", str(error.exception) + output.getvalue())

    def test_report_validation_repair_preserves_signed_thinking_without_reusing_evidence(self):
        claude = review.Claude(review.MODEL)
        report = {**REPORT, "findings": [FINDING]}
        signed = {"type": "thinking", "thinking": "PRIVATE", "signature": "SIGNED"}
        invalid = message([signed, *report_blocks(report)])
        read = message([{"type": "tool_use", "id": "read", "name": "read_file",
                         "input": {"path": "src/lib.rs", "revision": "head", "start": 8, "count": 1}}])
        responses = iter([invalid, read, message(report_blocks(report)), read, message(report_blocks(report))])
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            bodies.append(deepcopy(body))
            return next(responses)
        with patch.object(claude, "request", side_effect=request):
            self.assertEqual(claude.review("{}", review.Sources(FakeGitHub(), PR), PR["review_files"]), report)
        self.assertEqual(bodies[1]["messages"][1]["content"], invalid["content"])
        self.assertIn("anchor source line", bodies[1]["messages"][2]["content"])
        self.assertEqual(len(bodies[3]["messages"]), 1)

    def test_remaining_output_budget_caps_the_verifier_including_thinking(self):
        claude = review.Claude(review.MODEL)
        bodies = []
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            bodies.append(deepcopy(body))
            return message(report_blocks(REPORT), {"input_tokens": 10,
                                                  "output_tokens": 90 if len(bodies) == 1 else 10})
        with patch.object(review, "MAX_OUTPUT", 100), patch.object(claude, "request", side_effect=request):
            self.assertEqual(claude.review("{}", review.Sources(FakeGitHub(), PR), []), REPORT)
        self.assertEqual([b["max_tokens"] for b in bodies], [100, 10])
        self.assertEqual(claude.output_used, 100)

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

    def test_draft_and_closed_prs_are_skipped_for_every_review_trigger(self):
        events = {
            "pull_request_target": {"pull_request": {"number": 7}},
            "workflow_dispatch": {"inputs": {"pull_request": "7"}},
            "workflow_run": {"workflow_run": {"head_repository": {"full_name": review.REPOSITORY},
                                              "head_sha": PR["head"]["sha"]}},
        }
        for change in ({"draft": True}, {"state": "closed"}):
            for event_name, event in events.items():
                with self.subTest(change=change, event=event_name):
                    api = FakeGitHub()
                    api.pr.update(change)
                    with patch.object(review, "Claude") as claude:
                        numbers = review.targets(api, event, event_name)
                        for number in numbers:
                            review.review_pr(api, number, review.MODEL)
                    if event_name == "workflow_run":
                        self.assertEqual(numbers, [])
                    claude.assert_not_called()
                    self.assertEqual(api.writes, [])

    def test_marking_ready_reviews_a_pr_whose_ci_already_finished(self):
        api = FakeGitHub()
        api.pr["draft"] = True
        api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                     "conclusion": "success", "html_url": "https://example/ci"}]
        with patch.object(review.Claude, "review", return_value=deepcopy(REPORT)) as analysis:
            review.review_pr(api, 7, review.MODEL)
            analysis.assert_not_called()
            self.assertEqual(api.writes, [])
            api.pr["draft"] = False
            review.review_pr(api, 7, review.MODEL)
        analysis.assert_called_once()
        self.assertEqual(len([w for w in api.writes if w[1] == "issues/7/comments"]), 1)
        self.assertEqual(api.writes[-1][2]["state"], "success")

    def test_draft_transition_before_analysis_does_not_call_claude(self):
        api = FakeGitHub()
        sources = review.Sources(api, PR)
        api.pr["draft"] = True
        claude = review.Claude(review.MODEL)
        with patch.object(claude, "request") as request:
            with self.assertRaises(review.InactiveReview):
                claude.review("{}", sources, [])
        request.assert_not_called()

    def test_draft_or_close_during_token_count_stops_before_paid_inference(self):
        for change in ({"draft": True}, {"state": "closed"}):
            with self.subTest(change=change):
                api = FakeGitHub()
                api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                             "conclusion": "success", "html_url": "https://example/ci"}]
                calls = []
                def request(self, path, body):
                    calls.append(path)
                    api.pr.update(change)
                    return {"input_tokens": 10}
                with patch.object(review.Claude, "request", new=request):
                    review.review_pr(api, 7, review.MODEL)
                self.assertEqual(calls, ["messages/count_tokens"])
                self.assertEqual([w[2]["state"] for w in api.writes], ["pending"])

    def test_draft_transition_during_inference_stops_before_verification_and_publication(self):
        api = FakeGitHub()
        api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                     "conclusion": "success", "html_url": "https://example/ci"}]
        calls = []
        def request(self, path, body):
            calls.append(path)
            if path == "messages/count_tokens":
                return {"input_tokens": 10}
            api.pr["draft"] = True
            return message(report_blocks(REPORT))
        with patch.object(review.Claude, "request", new=request):
            review.review_pr(api, 7, review.MODEL)
        self.assertEqual(calls, ["messages/count_tokens", "messages"])
        self.assertEqual([w[2]["state"] for w in api.writes], ["pending"])

    def test_draft_transition_after_analysis_cannot_publish_a_comment_or_green_status(self):
        api = FakeGitHub()
        api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                     "conclusion": "success", "html_url": "https://example/ci"}]
        def analysis(*args):
            api.pr["draft"] = True
            return deepcopy(REPORT)
        with patch.object(review.Claude, "review", side_effect=analysis):
            review.review_pr(api, 7, review.MODEL)
        self.assertEqual([w[2]["state"] for w in api.writes], ["pending"])

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
                claude.return_value.usage_markdown.return_value = "API usage"
                review.review_pr(api, 7, review.MODEL)
            self.assertEqual(api.writes[-1][1], f"statuses/{PR['head']['sha']}")
            self.assertEqual(api.writes[-1][2]["state"], expected)
            self.assertEqual(len([w for w in api.writes if w[1] == "issues/7/comments"]), 1)

    def test_dollar_budget_stops_before_a_paid_messages_request(self):
        claude = review.Claude(review.MODEL)
        with patch.object(claude, "request", return_value={"input_tokens": 1_000_001}) as request:
            with self.assertRaisesRegex(ValueError, "Недостаточно бюджета"):
                claude.review("context", review.Sources(FakeGitHub(), PR), [])
        self.assertEqual([c.args[0] for c in request.call_args_list], ["messages/count_tokens"])

    def test_invalid_preflight_usage_stops_before_paid_inference(self):
        for count in (-1, True, "10", None):
            with self.subTest(count=count):
                claude = review.Claude(review.MODEL)
                with patch.object(claude, "request", return_value={"input_tokens": count}) as request:
                    with self.assertRaisesRegex(ValueError, "некорректную оценку"):
                        claude.review("{}", review.Sources(FakeGitHub(), PR), [])
                self.assertEqual([c.args[0] for c in request.call_args_list], ["messages/count_tokens"])

    def test_usage_separates_cache_costs_and_phases_without_logging_context(self):
        claude = review.Claude(review.MODEL)
        tokens = dict(zip(review.USAGE_FIELDS, (20, 30, 100, 5)))
        def request(path, body):
            if path == "messages/count_tokens":
                return {"input_tokens": 150}
            return message(report_blocks(REPORT), tokens)
        with patch.object(claude, "request", side_effect=request), \
                patch("sys.stdout", new_callable=io.StringIO) as output:
            claude.review("PRIVATE_SOURCE_SENTINEL", review.Sources(FakeGitHub(), PR), [])
        self.assertEqual(claude.usage, {"analysis": tokens, "verification": tokens})
        self.assertEqual(claude.input_used, 300)
        self.assertEqual(claude.output_used, 10)
        records = [json.loads(line) for line in output.getvalue().splitlines() if '"event": "claude_usage"' in line]
        self.assertEqual(len(records), 4)
        self.assertEqual([r["phase"] for r in records], ["analysis", "verification", "analysis", "verification"])
        self.assertAlmostEqual(records[0]["estimated_usd"], 0.00035)
        self.assertNotIn("PRIVATE_SOURCE_SENTINEL", output.getvalue())
        usage = claude.usage_markdown()
        self.assertIn("| Итого | 40 | 60 | 200 | 10 | 0.000700 |", usage)
        text = review.render(PR, REPORT, True, "CI", review.MODEL, usage)
        self.assertIn("### Расход API", text)
        self.assertIn("окончательный расход — в Claude Console", text)

    def test_usage_is_logged_even_when_the_model_cannot_finish(self):
        claude = review.Claude(review.MODEL)
        response = {"usage": {"input_tokens": 10, "output_tokens": 4},
                    "stop_reason": "max_tokens", "content": []}
        with patch.object(claude, "request", side_effect=[{"input_tokens": 10}, response]), \
                patch("sys.stdout", new_callable=io.StringIO) as output:
            with self.assertRaises(ValueError):
                claude.review("{}", review.Sources(FakeGitHub(), PR), [])
        records = [json.loads(line) for line in output.getvalue().splitlines() if '"event": "claude_usage"' in line]
        self.assertEqual(records[-2]["step"], "total")
        self.assertEqual(records[-2]["output_tokens"], 4)
        self.assertEqual(records[-1]["output_tokens"], 0)

    def test_failed_review_comment_includes_model_usage_cost_and_budget(self):
        api = self.successful_api()
        response = {"usage": {"input_tokens": 10, "cache_creation_input_tokens": 1000,
                              "cache_read_input_tokens": 10000, "output_tokens": 4},
                    "stop_reason": "max_tokens", "content": []}
        with patch.object(review.Claude, "request", side_effect=[{"input_tokens": 11010}, response]):
            with self.assertRaisesRegex(ValueError, "stop_reason=max_tokens"):
                review.review_pr(api, 7, review.MODEL)
        comments = [w[2]["body"] for w in api.writes if w[1] == "issues/7/comments"]
        self.assertEqual(len(comments), 1)
        self.assertIn("Проверка неполная", comments[0])
        self.assertIn(review.MODEL, comments[0])
        self.assertIn("| Итого | 10 | 1000 | 10000 | 4 | 0.007120 |", comments[0])
        self.assertIn("**$5.00**", comments[0])
        self.assertEqual(api.writes[-1][2]["state"], "error")

    def test_unknown_prices_and_invalid_usage_fail_before_a_clean_verdict(self):
        with self.assertRaisesRegex(ValueError, "тарифа"):
            review.Claude("unknown-model")
        for usage in ({"input_tokens": -1, "output_tokens": 1}, {"input_tokens": 10},
                      {"output_tokens": 1}, {"input_tokens": True, "output_tokens": 1}):
            with self.subTest(usage=usage):
                claude = review.Claude(review.MODEL)
                with patch.object(claude, "request", side_effect=[{"input_tokens": 10}, {"usage": usage}]):
                    with self.assertRaisesRegex(ValueError, "некорректный расход"):
                        claude.review("{}", review.Sources(FakeGitHub(), PR), [])

    def test_automatic_repeats_reuse_both_clean_and_blocking_completed_reviews(self):
        for report, expected in ((REPORT, "success"), ({**REPORT, "coverage_gaps": ["Missing caller"]}, "failure")):
            with self.subTest(state=expected):
                api = self.successful_api()
                with patch.object(review.Claude, "review", return_value=deepcopy(report)) as analysis:
                    review.review_pr(api, 7, review.MODEL)
                    original = deepcopy(api.status_records[0])
                    review.review_pr(api, 7, review.MODEL)
                analysis.assert_called_once()
                self.assertEqual(api.status_records[0]["state"], expected)
                self.assertEqual(api.status_records[0]["description"], original["description"])
                self.assertEqual(len([w for w in api.writes if w[1] == "issues/7/comments"]), 1)

    def test_manual_repeat_runs_again_and_appends_a_new_report(self):
        api = self.successful_api()
        with patch.object(review.Claude, "review", return_value=deepcopy(REPORT)) as analysis:
            review.review_pr(api, 7, review.MODEL)
            review.review_pr(api, 7, review.MODEL, force=True)
        self.assertEqual(analysis.call_count, 2)
        self.assertEqual(len([w for w in api.writes if w[1] == "issues/7/comments"]), 2)

    def test_changed_requirements_or_source_revisions_invalidate_completed_review(self):
        for mutation in ("issue", "head", "base", "body", "title", "model"):
            with self.subTest(mutation=mutation):
                api = self.successful_api()
                with patch.object(review.Claude, "review", return_value=deepcopy(REPORT)) as analysis:
                    review.review_pr(api, 7, review.MODEL)
                    model = review.MODEL
                    if mutation == "issue":
                        api.issue_text = "New acceptance criterion"
                    elif mutation in ("head", "base"):
                        api.pr[mutation]["sha"] = "d" * 40
                    elif mutation == "model":
                        model = "claude-sonnet-5-5"
                    else:
                        api.pr[mutation] += " updated"
                    review.review_pr(api, 7, model)
                self.assertEqual(analysis.call_count, 2)

    def test_controller_changes_invalidate_review_key(self):
        before = review.review_key("context", review.MODEL)
        with patch.object(Path, "read_bytes", return_value=b"changed controller"):
            self.assertNotEqual(review.review_key("context", review.MODEL), before)

    def test_completed_review_rejects_untrusted_wrong_and_incomplete_statuses(self):
        api = FakeGitHub()
        record = {"context": review.STATUS, "description": review.COMPLETED + "key", "state": "success",
                  "creator": {"login": "github-actions[bot]", "type": "Bot"}}
        for mutation in ({"state": "pending"}, {"state": "error"}, {"context": "ci-ok"},
                         {"description": review.COMPLETED + "other"}, {"creator": None},
                         {"creator": {"login": "attacker", "type": "User"}},
                         {"creator": {"login": "github-actions[bot]", "type": "User"}}):
            api.status_records = [{**record, **mutation}]
            self.assertIsNone(review.completed_review(api, PR, "key"))
        api.status_records = [record]
        self.assertEqual(review.completed_review(api, PR, "key"), record)

    def test_failed_ci_spends_nothing_and_successful_rerun_can_reuse_source_review(self):
        api = self.successful_api()
        api.runs[0]["conclusion"] = "failure"
        with patch.object(review, "Claude") as claude:
            review.review_pr(api, 7, review.MODEL, force=True)
        claude.assert_not_called()
        self.assertEqual(api.status_records[0]["state"], "failure")
        self.assertFalse(any(w[1] == "issues/7/comments" for w in api.writes))
        api.runs[0]["conclusion"] = "success"
        with patch.object(review.Claude, "review", return_value=deepcopy(REPORT)) as analysis:
            review.review_pr(api, 7, review.MODEL)
            api.runs[0]["conclusion"] = "failure"
            review.review_pr(api, 7, review.MODEL)
            api.runs[0]["conclusion"] = "success"
            review.review_pr(api, 7, review.MODEL)
        analysis.assert_called_once()
        self.assertEqual(api.status_records[0]["state"], "success")

    def test_ci_or_readiness_change_during_cache_lookup_cannot_restore_green(self):
        for mutation in ("pending", "failure", "draft", "head"):
            with self.subTest(mutation=mutation):
                api = self.successful_api()
                def lookup(*args):
                    if mutation == "pending":
                        api.runs[0]["status"] = "in_progress"
                    elif mutation == "failure":
                        api.runs[0]["conclusion"] = "failure"
                    elif mutation == "draft":
                        api.pr["draft"] = True
                    else:
                        api.pr["head"]["sha"] = "d" * 40
                    return {"state": "success"}
                with patch.object(review, "completed_review", side_effect=lookup), patch.object(review, "Claude") as claude:
                    review.review_pr(api, 7, review.MODEL)
                claude.assert_not_called()
                self.assertNotIn("success", [r["state"] for r in api.status_records])

    def test_source_or_requirement_changes_stop_before_the_next_paid_request(self):
        for field in ("head", "base", "title", "body"):
            api = FakeGitHub()
            sources = review.Sources(api, api.pr)
            claude = review.Claude(review.MODEL)
            def count(*args):
                if field in ("head", "base"):
                    api.pr[field]["sha"] = "d" * 40
                else:
                    api.pr[field] += " changed"
                return {"input_tokens": 10}
            with patch.object(claude, "request", side_effect=count) as request:
                with self.assertRaises(review.InactiveReview):
                    claude.review("{}", sources, [])
            self.assertEqual(request.call_count, 1)

    def test_target_output_and_matrix_execution_preserve_manual_force(self):
        with tempfile.TemporaryDirectory() as directory:
            event = Path(directory) / "event.json"
            output = Path(directory) / "output"
            event.write_text(json.dumps({"pull_request": {"number": 7}}))
            env = {"GITHUB_REPOSITORY": review.REPOSITORY, "GITHUB_TOKEN": "test",
                   "GITHUB_EVENT_PATH": str(event), "GITHUB_OUTPUT": str(output),
                   "GITHUB_EVENT_NAME": "pull_request_target", "REVIEW_PULL_REQUEST": "7"}
            with patch.dict(os.environ, env), patch.object(review, "ReviewGitHub", return_value=FakeGitHub()), \
                    patch("sys.argv", ["review", "targets"]):
                self.assertEqual(review.main(), 0)
            self.assertEqual(output.read_text(), "pull_requests=[7]\n")
            for name, force in (("pull_request_target", False), ("workflow_dispatch", True)):
                with patch.dict(os.environ, {**env, "GITHUB_EVENT_NAME": name}), \
                        patch.object(review, "ReviewGitHub", return_value=FakeGitHub()), \
                        patch.object(review, "review_pr") as run, patch("sys.argv", ["review", "run"]):
                    review.main()
                self.assertEqual(run.call_args.args[1:], (7, os.environ.get("CLAUDE_REVIEW_MODEL") or review.MODEL))
                self.assertEqual(run.call_args.kwargs, {"force": force})

    @staticmethod
    def successful_api():
        api = FakeGitHub()
        api.runs = [{"run_number": 1, "run_attempt": 1, "status": "completed",
                     "conclusion": "success", "html_url": "https://example/ci"}]
        return api

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
