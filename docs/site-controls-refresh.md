# Site controls refresh

This release spans the Rust backend and website assets. Deploy them together
using the normal merged-master workflow. The feature branch does not deploy.
No server changes or production data mutations are performed by the build.

## Owner VM resources

Owners and co-owners can enter CPU cores, memory in GB (stored as MB), and disk in GB from
VM Lab's resource dialog. These settings bypass the ordinary coin tiers and
session limits. Other users retain the existing tiers. Minimums are one core,
512 MB memory, and 40 GB disk; values must be finite whole numbers that fit the
host API. Physical host capacity still applies.

Existing computers must be stopped before editing. Storage can only grow.
The server serializes resource and power operations, applies host changes before
recording them, and reports host or persistence errors. A failed persistence
write can be completed by retrying the same values. No VM is forcibly stopped.

## Account search and staff links

Staff links live in the People page's right sidebar and follow each account's
roles. Admin account fields, ban searches, VM fleet search, and the main staff
search provide name, username, and email suggestions. Choosing a suggestion
passes the canonical email to existing actions. Keyboard arrows, Enter, and
Escape work in the list. Log-only searches retain their original behavior.

`GET /api/admin/account-search?q=...` requires staff authorization and the
existing passphrase gate. It returns at most twelve matches for queries of at
least two characters. Exact names resolve only when unique; ambiguous names
never silently select a person. Full Matrix IDs match the stored Matrix identity.
The owner-only account list uses `/api/admin/accounts`.

## Matrix direct conversations

People and Friends links pass the selected recipient to `/matrix/?dm=...`.
After verifying the existing chat device, the bootstrap opens the conversation
before starting Cinny, then navigates to `/matrix/direct/<room-id>/`.

`POST /api/matrix/dm` validates the Matrix bearer token, rejects banned accounts,
serializes room creation and account-data writes, and reuses joined two-person
DMs. New conversations are private and encrypted. Existing `m.direct` entries
are preserved. A retry can recover a created room if saving `m.direct` failed.
Recipients must have opened Chat once to establish their Matrix account; the
page explains this instead of opening a general room or claiming success.

## Design and cache changes

The homepage directory stays visible, preferences control the actual homepage
sections, and the school refresh preserves the existing page content, canonical
routing, and structured metadata. The school stylesheet keeps campus colors and
adds a one-column campus directory on phones. MitchCoin is a static SVG with no
animation or raster dependency. Updated asset versions and the service-worker
cache prevent mixing old design assets with the new release.

## Local validation

Run the workspace format, Clippy, and test commands from `rust/`. The browser
suite in `tests/test_site_controls.mjs` creates a disposable SQLite fixture and
mock Matrix server. It checks role gates, resource values above the ordinary
tiers, name suggestions, DM routing/reuse/retries, and layouts at 1440, 820, 390, and 320 pixels. It never uses live
accounts or the real VM host.
