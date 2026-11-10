# PR security screener

The `Security & Privilege Audit` job in `.github/workflows/pr-security-review.yml` runs on every PR (opened, synchronize, reopened) and leaves one comment that it updates in place. It is a static screener with no AI; the separate `AI Code Review` job in the same workflow adds its own comment.

The logic is in `tools/ci/pr_screen.py`. The workflow passes the PR's SHAs and author in as environment variables and does not interpolate them into shell.

## What it does

- Diffs against the **merge base** (`base...head`), so changes that landed on `master` after the branch was cut are not blamed on the PR.
- Sorts changed files into categories: admin/moderator lists, CI/CD workflows, deploy tooling, agent instructions (`AGENTS.md`), economy (coins/shop/casino), auth and access control, dependencies, application code, static content. All but the last three flag the PR.
- Scans only **added** lines for risky patterns: secrets and tokens, owner/privilege checks, dynamic execution, download-piped-to-shell, `unsafe`, SQL built with `format!`, raw HTML injection, `pull_request_target`, workflow secret use.
- Flags newly added `data/*.json` files (new storage must be a SQL table).
- Reports `file:line` and the rule name only. It never prints the matched line, so a leaked secret is not copied into the PR comment.
- Writes the report to the job summary too. PRs from forks get a read-only token and cannot be commented on, so the summary is the fallback.

## Limits

It is regex-based and advisory: the job always passes and does not block merging. A clean report is not a security review. To add a rule, append to `RULES` in the script.
