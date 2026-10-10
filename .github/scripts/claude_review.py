"""Review a pull request with the Claude Agent SDK.

The review workflow runs this from trusted main with the PR head checked out
in pr/. Claude Code investigates with read-only tools, following CLAUDE.md
and REVIEW.md from main, and the structured report becomes the step output
`result` for the publish job. The monthly API credits of Max and Team plans
cover the Agent SDK but not Claude Code GitHub Actions, hence this script.

Authentication is workload identity federation: the CLI exchanges the GitHub
OIDC token in ANTHROPIC_IDENTITY_TOKEN_FILE and re-reads that file whenever
it refreshes its access token, so a background task keeps the file fresh.
"""

import asyncio
import json
import os
from pathlib import Path
import sys
from urllib.request import Request, urlopen

import issue_labels
import review_findings

AUDIENCE = "https://api.anthropic.com"
TOKEN_REFRESH_SECONDS = 240
MAX_BUDGET_USD = 5.0
TOOLS = ["Read", "Grep", "Glob", "Agent", "Bash"]
ALLOWED_TOOLS = ["Read", "Grep", "Glob", "Agent",
                 "Bash(git -C pr diff *)", "Bash(git -C pr log *)", "Bash(git -C pr show *)",
                 "Bash(gh pr view *)", "Bash(gh pr diff *)", "Bash(gh issue view *)"]
# The classification an issue of a pre-existing finding is filed with
# (review_findings.py); the label catalog is the one vocabulary.
LABELS = [label["name"] for label in json.loads((Path(__file__).parents[1] / "labels.json").read_text())]
SCHEMA = {
    "type": "object", "additionalProperties": False, "required": ["summary", "findings"],
    "properties": {
        "summary": {"type": "string"},
        "findings": {"type": "array", "items": {
            "type": "object", "additionalProperties": False,
            "required": ["severity", "path", "line", "title", "body", "area", "priority", "issue"],
            "properties": {
                "severity": {"enum": ["important", "nit", "pre-existing"]},
                "path": {"type": "string"},
                "line": {"type": "integer", "minimum": 1},
                "title": {"type": "string"},
                "body": {"type": "string"},
                "area": {"enum": [name for name in LABELS if name.startswith("area:")]},
                "priority": {"enum": [name for name in LABELS if name.startswith("priority:")]},
                # The issue that already records a pre-existing finding, or 0.
                "issue": {"type": "integer", "minimum": 0}}}}}}


def write_identity_token(path, opener=urlopen):
    """Fetch a GitHub OIDC token for the Anthropic audience into path."""
    request = Request(f"{os.environ['ACTIONS_ID_TOKEN_REQUEST_URL']}&audience={AUDIENCE}",
                      headers={"Authorization": f"Bearer {os.environ['ACTIONS_ID_TOKEN_REQUEST_TOKEN']}"})
    with opener(request, timeout=30) as response:
        token = json.load(response)["value"]
    staged = path.with_name(path.name + ".new")
    staged.write_text(token)
    staged.chmod(0o600)
    staged.replace(path)


async def refresh_identity_token(path):
    while True:
        await asyncio.sleep(TOKEN_REFRESH_SECONDS)
        try:
            await asyncio.to_thread(write_identity_token, path)
        except Exception as error:  # The current token stays valid a while.
            print(f"::warning::OIDC token refresh failed: {type(error).__name__}", flush=True)


def failure(result):
    """Describe a run that produced no report, or None for a usable result."""
    if result is None:
        return "Claude Code ended without a result"
    if result.is_error or result.structured_output is None:
        status = result.api_error_status or "no API status"
        detail = result.result or "; ".join(result.errors or []) or "no message"
        return f"{result.subtype}, {status}: {detail}"
    return None


async def review(prompt, guidance, model, token_file):
    from claude_agent_sdk import ClaudeAgentOptions, ResultError, ResultMessage, query

    options = ClaudeAgentOptions(
        model=model, effort="high", max_budget_usd=MAX_BUDGET_USD,
        tools=TOOLS, allowed_tools=ALLOWED_TOOLS, permission_mode="dontAsk",
        # No repository hooks, permission rules or agents: guidance comes
        # only from the trusted files appended below.
        setting_sources=[],
        system_prompt={"type": "preset", "preset": "claude_code", "append": guidance},
        output_format={"type": "json_schema", "schema": SCHEMA})
    refresher = asyncio.create_task(refresh_identity_token(token_file))
    result = None
    try:
        async for message in query(prompt=prompt, options=options):
            if isinstance(message, ResultMessage):
                result = message
    except ResultError:
        # The SDK raises after yielding the failed ResultMessage; failure()
        # reports its subtype, API status and message.
        if result is None:
            raise
    finally:
        refresher.cancel()
    return result


def filed_findings(recorded):
    """The prompt's list of the findings already filed as issues, so
    the review names the issue of one it finds again instead of a new title."""
    issues = sorted({issue["number"]: issue for issue in recorded.values()}.values(),
                    key=lambda issue: issue["number"])
    if not issues:
        return "No finding has been filed as an issue yet."
    return "Findings already filed as issues:\n" + "\n".join(
        f"- #{issue['number']} ({issue['state']}): {issue['title']}" for issue in issues)


def main():
    pr, head = os.environ["REVIEW_PR"], os.environ["REVIEW_HEAD"]
    token_file = Path(os.environ["RUNNER_TEMP"]) / "anthropic-identity-token"
    write_identity_token(token_file)
    os.environ["ANTHROPIC_IDENTITY_TOKEN_FILE"] = str(token_file)
    guidance = "\n\n".join(Path(name).read_text() for name in ("CLAUDE.md", "REVIEW.md"))
    prompt = (f"Review pull request #{pr} of {os.environ['GITHUB_REPOSITORY']} at head {head}, "
              "checked out in pr/, following the review instructions.\n\n"
              + filed_findings(review_findings.recorded(issue_labels.GitHub(os.environ["GH_TOKEN"]))))
    result = asyncio.run(review(prompt, guidance, os.environ["CLAUDE_REVIEW_MODEL"], token_file))
    token_file.unlink(missing_ok=True)
    if reason := failure(result):
        print(f"::error::Claude review failed: {reason}")
        return 1
    print(f"Claude review: {result.num_turns} turns, estimated ${result.total_cost_usd or 0:.2f}")
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"result={json.dumps(result.structured_output, ensure_ascii=False)}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
