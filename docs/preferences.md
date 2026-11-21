# Preferences page

`/preferences/` (`webserver/preferences/index.html`, layout in `preferences-v2.css`)
shows one settings group at a time. A nav column on wide screens (a scrolling strip on
phones) switches between them, `#<tab>` in the URL opens one, and the search box
filters individual settings across every tab. A link to a section inside a tab (for
example `#data`) opens its tab: sections take their tab from `data-tab`, falling back
to their id.

| Tab | Sections |
| --- | --- |
| Appearance | theme, accent, contrast, density, corners, font, motion, holiday theme, material |
| Profile | profile fields, email change |
| Security | password, two-factor, passkeys, sessions |
| Privacy | online presence, activity sharing, stealth dashboard |
| Notifications | push/bell alert types, quiet hours, email (newsletter, blog alerts), ntfy |
| Homepage | homepage widgets |
| Games | game launch mode |
| Sync & data | sync, special tools, preference backup, game saves, account deletion |

The school page (`rjuhsd.school`, `/rjuhsd/preferences`) only has Appearance.

## How settings are stored

- Local settings are JSON in localStorage (`_prefPrivacy`, `_prefHomepage`,
  `_prefDisplay`, `_prefSync`, `_prefTools`, plus `theme_*` keys for appearance) and are
  synced by `sync.js`. Every control saves as you change it; the floating "Saved"
  message reports whether the sync reached the server.
- Notification settings are server-side (`GET/POST /api/me/notif-prefs`, see
  [notifications.md](notifications.md)); the Notifications tab and `/notifications/`
  edit the same record. Quiet hours need the browser's UTC offset, which the tab sends.
- Newsletter and blog alerts, ntfy, profile, email and security each use their own API.

## Only keep settings that do something

Every control must be read by something. When adding one, add its consumer in the same
change and list it above. Removed because nothing read them: "Activity Logging"
(`noHistory`), "Block Profile Lookups" (`blockProfile`) and the "Daily Bonus Plus"
toggle (the +50 coin bonus applies to owners automatically in `routes/daily_login.rs`).
Old stored values are ignored and dropped on the next save.
