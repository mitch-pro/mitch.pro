# Branch protection and code owners

How changes reach `master` (which deploys to production) and who must approve them.

## Rules on `master`

- **Pull request required.** `.github/CODEOWNERS` assigns only the sensitive paths to the `@mitch-pro/owners` team: `/.github/`, `tools/deploy.sh`, `AGENTS.md`, `SECURITY_POSTURE.md`, the build and hosting files (`rust/Dockerfile`, `docker-compose.yml`, `caddy/`), the security gates (`auth.rs`, `admin.rs`, `crypto.rs`, `mitch-server/src/handler.rs`, `webserver/admin-auth.js`) and the economy (`coins.rs`, `shop.rs`, `routes/casino.rs`, `routes/me/cosmetics.rs`). Do not add a catch-all `*` rule.
- **Code-owner review is enforced by a required status check, not by GitHub's native setting.** Required approvals is kept at 0 (so the owner can merge their own non-sensitive PRs — GitHub forbids authors from approving their own PR, so any value ≥ 1 would block that). Native "Require review from Code Owners" does nothing at 0 approvals, so enforcement is the **`Code owner gate`** check instead (see below).
- **Only the owner can push or merge to `master`** (push restriction), with admins included (`enforce_admins` on — the owner holds no branch-protection bypass right).
- **Required status checks: `rust-ci`, `email-check` and `Code owner gate`.** `rust-ci` is the job in `.github/workflows/rust-ci.yml` (format, clippy with warnings as errors, Rust tests, browser regressions). `email-check` likewise. The job/context name is what branch protection matches; do not rename without updating the rule.

### `Code owner gate`

`.github/workflows/codeowner-gate.yml` posts the `Code owner gate` commit status on every PR. It runs on `pull_request_target` and `pull_request_review`, which always execute the **default branch's** copy of the workflow, never a PR's edited copy, and it reads only PR metadata via the API (no checkout or execution of PR code). It:

- passes PRs that touch no CODEOWNERS path (everyday PRs merge solo);
- passes when the PR author is a code owner — the owner's own sensitive PRs go through on a normal merge, granted by the gate, without any bypass right;
- otherwise **fails** until a code owner *other than the author* has an approving review **on the current head commit** (a new commit invalidates a prior approval, like "dismiss stale reviews").

Who counts as a code owner is the **`CODE_OWNER_LOGINS`** repo variable (comma/space separated), kept in sync with `@mitch-pro/owners` by hand. If it is empty, or CODEOWNERS can't be read, the gate fails closed. This means a solo owner still cannot get a second-party review of their *own* sensitive changes — that is a logical limit (no one else to approve), not a gap; add a second owner to `CODE_OWNER_LOGINS` and the team to change it.
- The `Deploy to Production` workflow deploys `master` once `rust-ci` succeeds on the push. The `production` environment is limited to `master` and has **no required reviewers**, so every green master push deploys.

## Secrets

- SSH deploy secrets live in the `production` (master only) and `dev` (dev only) environments.
- `GEMINI_API_KEY` and `NTFY_*` for the AI review and push notifications live in the `automation` environment, which only `master` may use. The jobs that need them run from master: `push-ai-notify.yml` on master pushes, and `pr-ai-review.yml` as a `workflow_run` after `PR Security Review`, which GitHub always runs from master's copy. Don't add these as repo-level secrets again: any workflow pushed on any branch can read repo-level secrets.
- Secret scanning push protection is on.

## `dev`

`dev` is unprotected so collaborators can push to it. A push to `dev` deploys to the dev server (`deploy-dev` job in `deploy.yml`) over SSH, so anyone who can push to `dev` could run code on that host. The `dev` environment must have **required reviewers set to the `owners` team**: that approval is the only gate, because the job has no name check. If it is ever removed, every push to `dev` deploys unreviewed. Use a dev-only SSH key and host, and keep the SSH secrets in an environment limited to the branches that need them.

## Changing the rules

The protection rules live in GitHub settings, not in the repo. When they change, update this file in the same change.
