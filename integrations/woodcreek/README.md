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

The dashboard uses `public/index.html`, `woodcreek.css`, and `woodcreek.js`.
There is no inline controller or dependency on the old `components.css` file.
All backend modules must be copied: the runner, join flow, and selector registry
are now separate files imported by the manager. The existing Node runtime,
Playwright, and `ws` dependencies are sufficient; no Compose edit is needed.
Running `node backend/server.mjs` from this repository also finds the sibling
`public/` directory for local fixtures. The deployed flattened layout continues
to use its own `public/` directory.

The page reads `data-app-base` for mounted APIs and WebSockets. HTTP commands
are acknowledged and never replayed after an uncertain response. The live socket
only supplies shared state, logs, and heartbeat responses. Account images and
equipped cosmetics remain intact. Subdomains are separate applications.

The account provider is mitchdog.com. Deploy Mitch first, then copy `backend/*`
to the Node app root and rebuild the main app. The root stays read-only; no
additional secret, account database, mount, or Compose edit is required.

Public visits to woodcreek.site redirect to mitchdog.com/blooket-bot/. Mitch
supplies its scoped grant for HTML as well as API requests, so the backend serves
the proxied document without a redirect loop. Deploy the Mitch bridge first.
The tab uses Clever Portal and Clever's favicon; the interface keeps Tylerium
Services branding. The Server status navigation and panel have been removed.

Start answering / Stop answering controls the one server runner. Answering starts
off, and each connection shows whether it is paused, waiting, answering, or
waiting for game controls. Answer once does not enable continuous answering.
Retries and individual stops are available per connection. Tools uses the same
controller for auto answering, with separate single-bot script execution.

The modules, invariants, local test commands, and distinction between a Mitch
bridge deploy and a Woodcreek service deploy are documented in
[docs/blooket-bridge.md](../../docs/blooket-bridge.md). No test sends real game
joins, emails, or production commands.
