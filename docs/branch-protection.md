# Branch protection and code owners

How changes reach `master` (which deploys to production) and who must approve them.

## Rules on `master`

- **Pull request required**, with code-owner review. `.github/CODEOWNERS` assigns only the sensitive paths (`/.github/`, `tools/deploy.sh`, `AGENTS.md`, `auth.rs`) to the `@mitch-pro/owners` team, and required approvals is 0, so a PR touching only other paths can be merged by its author while one touching those paths needs a team member's approval. The team needs Write access to the repo. A PR author cannot approve their own PR. Do not add a catch-all `*` rule.
- **Required status check: `rust-ci`.** This is the `rust-ci` job in `.github/workflows/rust-ci.yml` (format, clippy with warnings as errors, Rust tests, browser regressions). The job name is what branch protection matches; do not rename it without updating the rule. The workflow runs on pushes and PRs targeting `master` and has no path filters, so the check always reports.
- The `Deploy to Production` workflow deploys `master` only after `rust-ci` succeeds, and only after a required reviewer approves the `production` environment (no name check in the workflow).

## `dev`

`dev` is unprotected so collaborators can push to it. A push to `dev` deploys to the dev server (`deploy-dev` job in `deploy.yml`) over SSH, so anyone who can push to `dev` could run code on that host. The `dev` environment must have **required reviewers set to the `owners` team**: that approval is the only gate, because the job has no name check. If it is ever removed, every push to `dev` deploys unreviewed. Use a dev-only SSH key and host, and keep the SSH secrets in an environment limited to the branches that need them.

## Changing the rules

The protection rules live in GitHub settings, not in the repo. When they change, update this file in the same change.
