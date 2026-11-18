#!/usr/bin/env bash
# Applies the branch rules for `main` with the GitHub CLI (`gh auth login` as the repository
# admin first). Run once; run again after renaming a CI job.
#
#   scripts/protect-main.sh [owner/repo]
#
# The rules:
#   - main only changes through a pull request;
#   - the checks "All green" (fmt, layering, tests) and "Corpus map" must pass, on a branch that
#     is up to date with main;
#   - a pull request needs an approving review from a code owner (.github/CODEOWNERS: Arun-murali0),
#     so nobody else can merge into main, and stale approvals are dismissed on new pushes;
#   - no force pushes, no deleting main, history stays linear (squash merges);
#   - the rules apply to administrators too, except that the auto-merge workflow's token (its
#     account is the owner) merges only commits authored by the owner, after its own gates.
set -euo pipefail

repo="${1:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}"
echo "protecting main on $repo"

gh api -X PUT "repos/$repo/branches/main/protection" --input - <<'JSON'
{
  "required_status_checks": {
    "strict": true,
    "contexts": ["All green", "Corpus map"]
  },
  "enforce_admins": false,
  "required_pull_request_reviews": {
    "required_approving_review_count": 1,
    "require_code_owner_reviews": true,
    "dismiss_stale_reviews": true
  },
  "restrictions": null,
  "required_linear_history": true,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "required_conversation_resolution": true
}
JSON
echo "done. Settings > Pages > Source: GitHub Actions turns on the docs site."
