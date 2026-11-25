# PR security screener

The `Security & Privilege Audit` job in `.github/workflows/pr-security-review.yml` runs on every PR (opened, synchronize, reopened) and leaves one comment that it updates in place. It is a static screener with no AI; the separate `AI Code Review` job in the same workflow adds its own comment.

The logic is in `tools/ci/pr_screen.py`. The workflow passes the PR's SHAs and author in as environment variables and does not interpolate them into shell.

## What it does

- Diffs against the **merge base** (`base...head`), so changes that landed on `master` after the branch was cut are not blamed on the PR.
- Sorts changed files into categories: admin/moderator lists, CI/CD workflows, deploy tooling, agent instructions (`AGENTS.md`), economy (coins/shop/casino), auth and access control, dependencies, application code, static content. All but the last three flag the PR.
- Scans only **added** lines for risky patterns: secrets and tokens, owner/privilege checks, dynamic execution, download-piped-to-shell, `unsafe`, SQL built with `format!`, raw HTML injection, `pull_request_target`, workflow secret use.
- Flags newly added `data/*.json` files (new storage must be a SQL table).
- Reports `file:line` and the rule name only. It never prints the matched line, so a leaked secret is not copied into the PR comment.
- Says an owner approval is required only when the PR changes a path listed in `.github/CODEOWNERS` (read from the base branch), because that's the only case where branch protection enforces one. Otherwise the report says its findings are advisory.
- Writes the report to the job summary too. PRs from forks get a read-only token and cannot be commented on, so the summary is the fallback.

## Limits

It is regex-based and advisory: the job always passes and does not block merging. A clean report is not a security review. To add a rule, append to `RULES` in the script.

## AI code review

The `AI Code Review` job (`.github/workflows/pr-ai-review.yml`) sends the PR to Gemini and posts one comment that it updates in place. It's meant to answer the owner's real question, whether merging is good for the site and its users, not just whether the code works. CI already checks that the code builds and the tests pass.

The prompt asks the model to judge, in order: user value (and added friction), direction and scope (parallel systems, duplicated pages or logic, changes that should be split), maintenance cost, breaches of the project rules, and then security, privacy and economy integrity. It answers with **Summary**, **Good for users**, **Concerns** and **Recommendation** (Merge, Merge with changes, Split or Don't merge). Merge is reserved for changes that are clearly a good direction, not just correct ones.

What the model gets:

- `AGENTS.md` and `docs/ui-design-standards.md` from the **base** branch, so a PR can't rewrite the rules it's judged against.
- The PR title and description, passed in through env vars and `jq --arg`, never interpolated into shell.
- The full list of changed files, plus the diff against the merge base, capped at 60 KB. When the diff is truncated, the model is told so and pointed to the file list.

Everything in the user message is marked as data, and the model is told not to follow instructions found in it. The review is advisory and never blocks a merge. The prompt is in `.github/workflows/pr-ai-review.yml`.

The job is a `workflow_run` that starts when `PR Security Review` finishes, not part of that `pull_request` workflow. That way it can use `GEMINI_API_KEY` and the ntfy credentials from the master-only `automation` environment: GitHub always runs master's copy of a `workflow_run` workflow, so a PR can't edit what runs with the secrets. The job looks up the PR from the run's head commit, checks out master, and fetches the PR head as git objects only, never checking it out or running it. It sends the "new PR" ntfy push and the review request only on a PR's first review. Because it runs from master, it shows on the Actions tab rather than in the PR's checks, and changes to it only take effect once merged.

