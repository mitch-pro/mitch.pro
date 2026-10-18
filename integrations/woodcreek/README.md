# Tylerium Services on woodcreek.site

These are the redesigned files for the main `woodcreek.site` application.
The `backend/` directory versions the account middleware, Blooket manager, and
Node HTTP/WebSocket entrypoint; deploy these files at the application root. Other
public assets remain on `tyler-home-server`; subdomains are separate applications.

The live app is Docker Compose service `app` in
`/opt/recovered-sites/woodcreek`, container `recovered-woodcreek-app-1`.
Its image is `recovered-woodcreek-app`. The container has a read-only root
filesystem and no bind-mounted public directory. The older source mirror is
`/home/tbot5k/Websites/woodcreek.site`.

Back up the current public files outside the webroot and tag the existing image
for rollback. Copy these files into the build context and source mirror. From
the Compose directory, build with `docker compose -p recovered-woodcreek build app`,
then publish with `docker compose -p recovered-woodcreek up -d --no-deps --no-build app`.
The project name must match the existing deployment so the correct image and
container are updated. This restarts only the main application; check active
sessions first. Keep other services and Compose settings unchanged. The app uses a process limit of
1024; its previous limit of 512 caused Chromium thread creation failures.

`components.css` preserves the existing advanced controls and dialogs;
`woodcreek.css` supplies the neutral responsive layout. School branding and
student-themed labels have been removed. Interface sound defaults to off and
respects the saved preference. The bot controller remains in `index.html`.

The app reads `data-app-base` for mounted API and WebSocket URLs. It runs at
Woodcreek's root and through Mitch's `/blooket-bot/` bridge. Incoming settings
update controls when they are not being edited; hydration never sends settings
back to the server. See [the bridge documentation](../../docs/blooket-bridge.md).

The account provider is mitchdog.com. Deploy Mitch first, then copy `backend/*`
to the Node app root and rebuild the main app. The root stays read-only; no
additional secret, account database, mount, or Compose edit is required.

Public visits to woodcreek.site redirect to mitchdog.com/blooket-bot/. Mitch
supplies its scoped grant for HTML as well as API requests, so the backend serves
the proxied document without a redirect loop. Deploy the Mitch bridge first.
The tab uses Clever Portal and Clever's favicon; the interface keeps Tylerium
Services branding. The Server status navigation and panel have been removed.

The visible Start answering / Stop answering button controls the server's
answering state and updates from other clients. Answering starts off. One
controller submits each bot's own question once; it no longer runs a second
40ms loop or broadcasts one bot's answer to every other bot.

All joins and retries share two slots with 400ms spacing and checks for process
and memory headroom. At most 50 connections may be active or queued across the
service, and names stay unique within a game across batches. Stopping cancels
queued work. Games already running can be joined directly. Disconnections and
explicit host removals have separate diagnostics and do not trigger silent
name changes or automatic rejoining after removal.

The Blooket controller is split by responsibility:

- `blooket-settings.mjs` owns defaults and validates complete settings patches
  before applying them. Launching preserves unspecified settings; accuracy edits
  recalculate targets for existing bots.
- `blooket-commands.mjs` dispatches both HTTP and WebSocket commands. Script files
  are confined to the real `public/cheats` directory, including symlink checks.
- `blooket-screen.mjs` reads visible controls and the matching question's data.
  Question wording cannot trigger a minigame, and hidden stale controls are ignored.
- `blooket-answers.mjs` chooses and submits one answer per observed question.
  Unknown multiple-choice keys may be guessed but are excluded from accuracy;
  typing questions with no key wait. Typing and click-position options affect the
  actual input controls. A feedback screen resets the question submission lock.
- `blooket-minigames.mjs` performs one visible action per poll. Chest selection is
  random or positional; player targeting uses the first listed player or a random
  player. The UI no longer promises unavailable chest previews, crypto defenses,
  or fishing depth settings.
- `blooket-names.mjs` keeps roster names and avatar constants outside the controller.
- `blooket-manager.mjs` owns browser lifetimes, reservations, retries, and polling.
  Finished games release contexts, closed pages become errors, and concurrent
  stops share one cleanup operation. Stopping also pauses answering and clears
  question counters. Removed server telemetry is no longer collected per event.

Tools that start answering or fishing on bots use the same server controller as
the dashboard; they do not inject a second background solver.

Scripts use real game state when available and report unavailable or failed
execution without fabricating a state node, database writes, or scores.

Run `npm run test:blooket` from the repository root for browser fixtures covering
these behaviors without connecting to Blooket. The mounted bridge and account
handoff additionally require the local Rust server binary:
`node tests/test_blooket_bridge_browser.mjs` and
`node tests/test_woodcreek_accounts.mjs`.
