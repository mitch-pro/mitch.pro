# Branch protection and code owners

How changes reach `master` (which deploys to production) and who must approve them.

## Rules on `master`

- **Pull request required**, with code-owner review. `.github/CODEOWNERS` assigns only the sensitive paths to the `@mitch-pro/owners` team: `/.github/`, `tools/deploy.sh`, `AGENTS.md`, `SECURITY_POSTURE.md`, the build and hosting files (`rust/Dockerfile`, `docker-compose.yml`, `caddy/`), the security gates (`auth.rs`, `admin.rs`, `crypto.rs`, `mitch-server/src/handler.rs`, `webserver/admin-auth.js`) and the economy (`coins.rs`, `shop.rs`, `routes/casino.rs`, `routes/me/cosmetics.rs`). Required approvals is 0, so a PR touching only other paths can be merged by its author, while one touching those paths needs a team member's approval. The team needs Write access to the repo. A PR author cannot approve their own PR. Do not add a catch-all `*` rule.
- **Only the owner can push or merge to `master`** (push restriction), with admins included.
- **Required status checks: `rust-ci` and `email-check`**, both from GitHub Actions. `rust-ci` is the job in `.github/workflows/rust-ci.yml` (format, clippy with warnings as errors, Rust tests, browser regressions). The job name is what branch protection matches; do not rename it without updating the rule. The workflow runs on pushes and PRs targeting `master` and has no path filters, so the check always reports.
- The `Deploy to Production` workflow deploys `master` once `rust-ci` succeeds on the push. The `production` environment is limited to `master` and has **no required reviewers**, so every green master push deploys.

## Secrets

- SSH deploy secrets live in the `production` (master only) and `dev` (dev only) environments.
- `GEMINI_API_KEY` and `NTFY_*` for the AI review and push notifications live in the `automation` environment, which only `master` may use. The jobs that need them run from master: `push-ai-notify.yml` on master pushes, and `pr-ai-review.yml` as a `workflow_run` after `PR Security Review`, which GitHub always runs from master's copy. Don't add these as repo-level secrets again: any workflow pushed on any branch can read repo-level secrets.
- Secret scanning push protection is on.

## `dev`

`dev` is unprotected so collaborators can push to it. A push to `dev` deploys to the dev server (`deploy-dev` job in `deploy.yml`) over SSH, so anyone who can push to `dev` could run code on that host. The `dev` environment must have **required reviewers set to the `owners` team**: that approval is the only gate, because the job has no name check. If it is ever removed, every push to `dev` deploys unreviewed. Use a dev-only SSH key and host, and keep the SSH secrets in an environment limited to the branches that need them.

## Changing the rules

The protection rules live in GitHub settings, not in the repo. When they change, update this file in the same change.
