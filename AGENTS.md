# Project instructions

- Commit every completed change to mitchdog.com in this repository.
- Use natural, concise interface copy. Prefer direct labels that describe the content or action.
- Do not add marketing slogans, decorative eyebrow headings, motivational filler, or promotional helper text to ordinary site pages. Do not add copy merely to fill space.
- Keep layouts focused on the page's function. Preserve users' chosen profile photos and equipped cosmetics when displaying their identities.

## Production server access

- Never SSH into the production server on your own initiative, for any reason — not to check logs, not to "quickly" fix something, not because a deploy looks stuck or failed. This applies every single time, with no exceptions, regardless of how routine or low-risk the action seems.
- Deploys happen through the `Deploy to Production` GitHub Actions workflow (triggered by a push to `master`) or `tools/deploy.sh` run manually by a human. Do not invoke either yourself by SSHing in and running it.
- If something on the server needs inspecting or fixing, stop and tell the person you're working with exactly what you want to check or run, and wait for them to say yes before doing it. Give them the specific command, not a vague description.
- This holds even if you already have working SSH access or credentials. Having access is not the same as having permission for this specific action right now.
