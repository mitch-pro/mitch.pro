# Woodcreek dashboard frontend

These are the redesigned files for the main `woodcreek.site` application.
The existing Node backend and its other public assets stay on
`tyler-home-server`; subdomains are separate applications.

The live app is Docker Compose service `app` in
`/opt/recovered-sites/woodcreek`, container `recovered-woodcreek-app-1`.
Its image is `recovered-woodcreek-app`. It has no bind-mounted public directory,
so an update must change both the build context and the running container.
The older source mirror is `/home/tbot5k/Websites/woodcreek.site`.

Back up the current public files outside the webroot, copy these files into
the build context and source mirror, and run `docker compose build app` from
the Compose directory. Copy the same files into `/app/public/` in the running
container. Static responses are read from disk, so the app needs no restart.
Do not change the Compose configuration or operate on other services.

`components.css` preserves the existing advanced controls and dialogs;
`woodcreek.css` supplies the neutral responsive layout. School branding and
student-themed labels have been removed. Interface sound defaults to off and
respects the saved preference. The bot controller remains in `index.html`.

The app reads `data-app-base` for mounted API and WebSocket URLs. It runs at
Woodcreek's root and through Mitch's `/blooket-bot/` bridge. Incoming settings
update controls when they are not being edited; hydration never sends settings
back to the server. See [the bridge documentation](../../docs/blooket-bridge.md).
