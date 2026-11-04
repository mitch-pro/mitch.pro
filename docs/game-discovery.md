# Games library

`/game-portal/` is the current catalog and player. Exact legacy catalog URLs
(`/games`, `/games/`, and their index aliases) redirect there with their query
string intact. Individual `/games/<slug>/` files and `/api/games` retain their
existing behavior. Home links use `/` and `target="_top"` so a library opened
inside the homepage's frame returns to the actual homepage.

## Popularity order

`FEATURED_TITLES` in `rust/crates/mitch-server/src/routes/portal_catalog.rs` is the single maintained ordering. The
initial selection was reviewed on October 21, 2026, using publishers' own lists:

- [Coolmath's popular games](https://www.coolmathgames.com/blog/top-10-most-popular-games-on-coolmath-games/body)
  identifies Run 3, Drift Boss, Fireboy and Watergirl, Moto X3M, Tiny Fishing,
  Papa's Pizzeria, Eggy Car, and Cookie Clicker, among others.
- [Poki's popular games](https://poki.com/en/popular) includes Retro Bowl,
  Subway Surfers, Drift Boss, Stickman Hook, Monkey Mart, Drive Mad, and Paper.io 2.

- [Hooda Math's school-focused recommendations](https://www.hoodamath.com/blog/best-unblocked-games-to-play-at-school-in-2026-no-downloads-needed/)
  highlight Snow Rider 3D, OvO, Drift Hunters, and Eggy Car.

These are publisher-provided popularity signals, not a Mitchdog or RJUHSD usage
ranking. Combining them for this audience is an editorial choice. Familiar
existing catalog games follow these picks.
Unavailable titles are not added, and similarly named clones are not boosted by
substring matching. The list can be revised without changing the game catalog.

The default catalog and the twelve-game popular shelf use the same title rank.
Title keys ignore spaces and punctuation so `MotoX3M` matches `Moto X3M`.
For duplicate titles, an available local game precedes an external copy; missing
artwork can reuse the existing image for the same title.
Unranked titles follow alphabetically. A–Z is an explicit alternate sort;
category and search filters retain the selected ordering. Popular tiles are
hidden during a search, category filter, or personal-library view.

IDs are assigned in the original catalog order before sorting, preserving saved
game IDs. Popular tiles collapse duplicate title keys without deleting catalog
entries or saved games. No new play tracking, analytics, account storage, or
server-side popularity counters are introduced. Existing reward authorization,
heartbeat spacing, caps, and wallet accounting remain unchanged.

## Retro console emulation

`/games/retro/` provides an in-browser WebAssembly console player powered by
EmulatorJS for PlayStation 1 (PS1), Nintendo 64 (N64), Game Boy Advance (GBA),
Super Nintendo (SNES), Nintendo Entertainment System (NES), Game Boy / Game Boy
Color (GB/GBC), Sega Genesis / Mega Drive (MD), Sega Master System / Game Gear,
and Atari 2600.

The player supports drag-and-drop loading of user-provided ROM files and saves state
locally in IndexedDB. Legal public-domain and open-source homebrew demo games are
provided for instant play. Direct console links (e.g. `/games/retro/?console=psx`)
are cataloged in `/game-portal/catalog.json` under the Retro genre.

## Navigation and verification

Embedded games retain the player with separate **Back to games** and **Home**
actions. Closing a game restores the catalog and search controls. External games
are labeled as opening in another tab. The sign-in link returns to the library
after authentication.


## How the portal gets its list

The catalog (`data/portal_catalog.json`, about 8 MB) is not in the web root and
the browser never downloads it. `routes/portal_catalog.rs` loads it once and
serves only what the page shows: `/api/portal/summary` (total, category counts and
the featured row), `/api/portal/list?cat=&q=&sort=&offset=&limit=` (one page of
60), `/api/portal/items?ids=` (saved and recent items) and `/api/portal/random`.
Search, category filtering and both sorts run on the server. Ids are the same
numbering as before, so saved and recent lists keep working. The legacy catalog URLs (`/games/`,
`/games/index.html`, ...) redirect to `/game-portal/` only while
`GAMES_LIBRARY_REDIRECT_ENABLED` in `handler.rs` is true; it is on. Set it to false to make
`/games/` serve its own page again.

## The /portal/ page

`/portal/` (`webserver/portal/`) is a second front end for the same library,
written separately from `/game-portal/`. It uses the shared masthead and theme
tokens, so it follows the site's light and dark themes. It reads only
`/api/portal/*`, loading 48 titles at a time as the grid scrolls, and every game,
thumbnail and file it loads goes through the short `/prox/` paths. It keeps the
older page's saved/recent storage keys, so those lists carry over, and uses the
same `/api/game-portal/status` and `/heartbeat` endpoints for MitchCoin rewards
(the reward rules are unchanged). It is public like `/game-portal/`: `/portal` is
in `HTML_OPEN`, the password-enforcement exemptions and the public-assets gate in
`handler.rs`.
