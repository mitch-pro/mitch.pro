# Games library

`/game-portal/` is the current catalog and player. Exact legacy catalog URLs
(`/games`, `/games/`, and their index aliases) redirect there with their query
string intact. Individual `/games/<slug>/` files and `/api/games` retain their
existing behavior. Home links use `/` and `target="_top"` so a library opened
inside the homepage's frame returns to the actual homepage.

## Popularity order

`webserver/game-portal/ui/popularity.js` is the single maintained ordering. The
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
are cataloged in `/game-portal/games.json` under the Retro genre.

## Navigation and verification

Embedded games retain the player with separate **Back to games** and **Home**
actions. Closing a game restores the catalog and search controls. External games
are labeled as opening in another tab. The sign-in link returns to the library
after authentication.

After building `mitch-server`, run:

```sh
node tests/test_game_portal_unit.js
node tests/test_game_discovery_browser.mjs
```

The browser fixture runs a temporary local server and mocks remote artwork,
games, and reward responses. It checks ranking, full-catalog access, filtering,
saved games, player cleanup, homepage navigation, legacy redirects, and mobile
layouts. It never plays games or sends messages against production.
