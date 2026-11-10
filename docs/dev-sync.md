# dev branch sync

`.github/workflows/sync-dev.yml` runs on every push to `master` and resets `dev` to `master`.

| State of `dev` | Result |
| :--- | :--- |
| missing | created from `master` |
| equal to `master` | nothing |
| behind `master` (no commits `master` lacks) | overwritten with `master`, no backup |
| same content as `master`, different history (e.g. squash-merged) | overwritten, no backup |
| ahead of or diverged from `master` | current tip saved to `backup/dev-<UTC timestamp>-<short sha>`, then `dev` overwritten |

The overwrite uses `--force-with-lease` on the sha the job inspected, so a push to `dev` during the run fails the job instead of being lost. Runs are serialized (`concurrency: sync-dev`).

## Token

By default the job uses `GITHUB_TOKEN`. Two limits follow from that:

- GitHub rejects a push containing changes to `.github/workflows/*` from `GITHUB_TOKEN`. When a master push changes a workflow, the sync fails until `dev` is updated by hand.
- Pushes made with `GITHUB_TOKEN` do not trigger other workflows, so `deploy-dev` does not redeploy after a sync.

Adding a `DEV_SYNC_TOKEN` repo secret (a PAT with `contents` and `workflow` write access) fixes both; the workflow uses it when present.

Delete old `backup/dev-*` branches when they are no longer needed.
