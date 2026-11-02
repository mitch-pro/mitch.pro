# Branch protection and code owners

How changes reach `master` (which deploys to production) and who must approve them.

## Rules on `master`

- **Pull request required**, with code-owner review. `.github/CODEOWNERS` assigns every path to the owner (`@urashil`), so only the owner's approval counts. A PR author cannot approve their own PR.
- **Required status check: `rust-ci`.** This is the `rust-ci` job in `.github/workflows/rust-ci.yml` (format, clippy with warnings as errors, Rust tests, browser regressions). The job name is what branch protection matches; do not rename it without updating the rule. The workflow runs on pushes and PRs targeting `master` and has no path filters, so the check always reports.
- The `Deploy to Production` workflow deploys `master` only after `rust-ci` succeeds, and only when the pushing actor is the owner.

## `dev`

`dev` is unprotected so collaborators can push to it. A push to `dev` deploys to the dev server (`deploy-dev` job in `deploy.yml`) over SSH, so anyone who can push to `dev` can run code on that host. Use a dev-only SSH key and host, and keep the SSH secrets in an environment limited to the branches that need them.

## Changing the rules

The protection rules live in GitHub settings, not in the repo. When they change, update this file in the same change.
