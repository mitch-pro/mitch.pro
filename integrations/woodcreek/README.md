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
sessions first. Do not change Compose configuration or operate on other services.

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
