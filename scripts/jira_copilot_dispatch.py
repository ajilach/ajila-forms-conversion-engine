#!/usr/bin/env python3
"""Bridge Jira tickets labelled `copilot` to the GitHub Copilot coding agent.

For every open Jira issue carrying the configured label this script:

1. Creates a GitHub issue mirroring the Jira summary/description (with a back-link).
2. Assigns the GitHub Copilot coding agent to that issue (Copilot then opens its own
   draft pull request and works on the change).
3. Comments back on the Jira ticket with the GitHub issue link and removes the label
   so the same ticket is not picked up again on the next run (idempotency).

Configuration comes entirely from environment variables so the GitHub Actions workflow
can inject repository secrets without touching the code:

    JIRA_BASE_URL   e.g. https://ajila.atlassian.net   (required)
    JIRA_EMAIL      Atlassian account email             (required)
    JIRA_TOKEN      Atlassian API token                 (required)
    GITHUB_PAT      user PAT with issues, pull_requests, contents and actions
                    read/write (a fine-grained PAT scoped to the target repo); the
                    default GITHUB_TOKEN cannot assign Copilot (required)
    GITHUB_REPO     owner/repo, e.g. ajilach/blueprint-app (required)
    BASE_BRANCH     branch Copilot should base its PR on (default: master)
    COPILOT_LABEL   Jira label that triggers dispatch    (default: copilot)
    JIRA_PROJECT    optional project key to scope the JQL

Pass --dry-run to only print the matching Jira tickets without creating anything.

Uses only the Python standard library — no third-party dependencies.
"""

from __future__ import annotations

import base64
import json
import os
import sys
import urllib.error
import urllib.request

# Login of the Copilot coding agent bot, returned by the GraphQL suggestedActors query.
COPILOT_LOGIN = "copilot-swe-agent"
# Required to opt into the Copilot assignment fields of the GraphQL API.
GRAPHQL_FEATURES = "issues_copilot_assignment_api_support"


class ConfigError(Exception):
    """Raised when required configuration is missing — aborts the whole run."""


# --------------------------------------------------------------------------- #
# Low-level HTTP helpers
# --------------------------------------------------------------------------- #
def _request(method: str, url: str, headers: dict, body: dict | None = None) -> dict:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req) as resp:
            raw = resp.read()
            return json.loads(raw) if raw else {}
    except urllib.error.HTTPError as exc:
        detail = exc.read().decode(errors="replace")
        raise RuntimeError(f"{method} {url} -> HTTP {exc.code}: {detail}") from exc


# --------------------------------------------------------------------------- #
# Jira
# --------------------------------------------------------------------------- #
class Jira:
    def __init__(self, base_url: str, email: str, token: str):
        self.base_url = base_url.rstrip("/")
        creds = base64.b64encode(f"{email}:{token}".encode()).decode()
        self.headers = {
            "Authorization": f"Basic {creds}",
            "Content-Type": "application/json",
            "Accept": "application/json",
        }

    def search(self, jql: str) -> list[dict]:
        body = {
            "jql": jql,
            "fields": ["summary", "description", "labels"],
            "maxResults": 50,
        }
        result = _request(
            "POST", f"{self.base_url}/rest/api/3/search/jql", self.headers, body
        )
        return result.get("issues", [])

    def add_comment(self, key: str, text: str) -> None:
        # The v3 comment endpoint expects an Atlassian Document Format (ADF) body.
        body = {
            "body": {
                "type": "doc",
                "version": 1,
                "content": [
                    {
                        "type": "paragraph",
                        "content": [{"type": "text", "text": text}],
                    }
                ],
            }
        }
        _request(
            "POST",
            f"{self.base_url}/rest/api/3/issue/{key}/comment",
            self.headers,
            body,
        )

    def remove_label(self, key: str, label: str) -> None:
        body = {"update": {"labels": [{"remove": label}]}}
        _request(
            "PUT", f"{self.base_url}/rest/api/3/issue/{key}", self.headers, body
        )


def adf_to_text(node) -> str:
    """Flatten an Atlassian Document Format node tree to plain text.

    Good enough to give Copilot the ticket context; full markdown export is overkill.
    """
    if node is None:
        return ""
    if isinstance(node, str):
        return node
    if isinstance(node, list):
        return "".join(adf_to_text(n) for n in node)

    node_type = node.get("type")
    if node_type == "text":
        return node.get("text", "")
    if node_type == "hardBreak":
        return "\n"

    text = adf_to_text(node.get("content"))
    # Block-level nodes get a trailing newline so paragraphs/list items stay separated.
    if node_type in ("paragraph", "heading", "listItem", "blockquote", "codeBlock"):
        text += "\n"
    return text


# --------------------------------------------------------------------------- #
# GitHub
# --------------------------------------------------------------------------- #
class GitHub:
    def __init__(self, token: str, repo: str):
        self.repo = repo
        self.owner, self.name = repo.split("/", 1)
        self.rest_headers = {
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "Content-Type": "application/json",
            "X-GitHub-Api-Version": "2022-11-28",
        }
        self.graphql_headers = {
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "GraphQL-Features": GRAPHQL_FEATURES,
        }
        self._repo_id: str | None = None
        self._copilot_id: str | None = None

    def _graphql(self, query: str, variables: dict) -> dict:
        result = _request(
            "POST",
            "https://api.github.com/graphql",
            self.graphql_headers,
            {"query": query, "variables": variables},
        )
        if result.get("errors"):
            raise RuntimeError(f"GraphQL errors: {json.dumps(result['errors'])}")
        return result["data"]

    def create_issue(self, title: str, body: str) -> dict:
        """Create an issue and return {number, html_url, node_id}."""
        return _request(
            "POST",
            f"https://api.github.com/repos/{self.repo}/issues",
            self.rest_headers,
            {"title": title, "body": body},
        )

    def _resolve_ids(self) -> tuple[str, str]:
        """Resolve (repository node id, Copilot bot node id), cached per run.

        Raises if Copilot is not available as an assignee on this repo — that almost
        always means the coding agent is not enabled for the repository/org.
        """
        if self._repo_id and self._copilot_id:
            return self._repo_id, self._copilot_id
        query = """
        query($owner:String!, $name:String!) {
          repository(owner:$owner, name:$name) {
            id
            suggestedActors(capabilities:[CAN_BE_ASSIGNED], first:100) {
              nodes { login __typename ... on Bot { id } ... on User { id } }
            }
          }
        }
        """
        repo = self._graphql(query, {"owner": self.owner, "name": self.name})["repository"]
        self._repo_id = repo["id"]
        for node in repo["suggestedActors"]["nodes"]:
            if node.get("login") == COPILOT_LOGIN:
                self._copilot_id = node["id"]
                return self._repo_id, self._copilot_id
        raise RuntimeError(
            f"'{COPILOT_LOGIN}' not assignable on {self.repo} — is the Copilot "
            "coding agent enabled for this repository?"
        )

    def assign_copilot(self, issue_node_id: str, base_branch: str) -> None:
        # baseRef tells Copilot which branch to base its PR on; without agentAssignment
        # it would default to the repository's default branch.
        repo_id, copilot_id = self._resolve_ids()
        mutation = """
        mutation($assignableId:ID!, $actorIds:[ID!]!, $repoId:ID!, $baseRef:String!) {
          replaceActorsForAssignable(input:{
            assignableId:$assignableId,
            actorIds:$actorIds,
            agentAssignment:{targetRepositoryId:$repoId, baseRef:$baseRef}
          }) {
            assignable { __typename }
          }
        }
        """
        self._graphql(
            mutation,
            {
                "assignableId": issue_node_id,
                "actorIds": [copilot_id],
                "repoId": repo_id,
                "baseRef": base_branch,
            },
        )


# --------------------------------------------------------------------------- #
# Orchestration
# --------------------------------------------------------------------------- #
def env(name: str, default: str | None = None, required: bool = False) -> str:
    value = os.environ.get(name, default)
    if required and not value:
        raise ConfigError(f"Missing required environment variable: {name}")
    return value or ""


def build_jql(label: str, project: str) -> str:
    clauses = [f'labels = "{label}"', "statusCategory != Done"]
    if project:
        clauses.append(f'project = "{project}"')
    return " AND ".join(clauses) + " ORDER BY created ASC"


def process_ticket(jira: Jira, gh: GitHub, issue: dict, base_branch: str, label: str) -> None:
    key = issue["key"]
    fields = issue.get("fields", {})
    summary = fields.get("summary", key)
    description = adf_to_text(fields.get("description")).strip()

    body = (
        f"{description}\n\n" if description else ""
    ) + f"---\nQuelle: {jira.base_url}/browse/{key}\nBase-Branch: `{base_branch}`"
    title = f"[{key}] {summary}"

    gh_issue = gh.create_issue(title, body)
    gh.assign_copilot(gh_issue["node_id"], base_branch)
    print(f"  -> created {gh_issue['html_url']} and assigned Copilot")

    jira.add_comment(
        key,
        f"GitHub-Copilot wurde beauftragt: {gh_issue['html_url']} "
        f"(Issue #{gh_issue['number']}). Der Coding-Agent erstellt automatisch einen Pull Request.",
    )
    jira.remove_label(key, label)
    print(f"  -> commented on {key} and removed label '{label}'")


def main() -> int:
    dry_run = "--dry-run" in sys.argv

    try:
        jira = Jira(
            env("JIRA_BASE_URL", required=True),
            env("JIRA_EMAIL", required=True),
            env("JIRA_TOKEN", required=True),
        )
        label = env("COPILOT_LABEL", "copilot")
        project = env("JIRA_PROJECT")
        base_branch = env("BASE_BRANCH", "master")
        if not dry_run:
            gh = GitHub(env("GITHUB_PAT", required=True), env("GITHUB_REPO", required=True))
    except ConfigError as exc:
        print(f"Configuration error: {exc}", file=sys.stderr)
        return 1

    jql = build_jql(label, project)
    print(f"Querying Jira: {jql}")
    issues = jira.search(jql)
    print(f"Found {len(issues)} ticket(s) with label '{label}'.")

    if dry_run:
        for issue in issues:
            print(f"  {issue['key']}: {issue.get('fields', {}).get('summary', '')}")
        return 0

    failures = 0
    for issue in issues:
        key = issue["key"]
        print(f"Processing {key} ...")
        try:
            process_ticket(jira, gh, issue, base_branch, label)
        except Exception as exc:  # one bad ticket must not block the rest
            failures += 1
            print(f"  !! failed to process {key}: {exc}", file=sys.stderr)

    if failures:
        print(f"Completed with {failures} failed ticket(s).", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
