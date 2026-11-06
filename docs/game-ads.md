# Ads inside proxied games

The portal serves games from other hosts (`/prox/gamemonetize/`, `/prox/luma/`,
`/prox/calculated2/`, by id as `/prox/g/<id>/`). Many of those games come with the
ads their publishers run, and **mitch.pro earns nothing from them**.

## Why this matters

- **Students can be shown explicit or unsuitable ads.** The ads are chosen by third
  party ad networks, not by us, and they can't be reviewed in advance. Whoever runs
  the site is responsible for what students are shown on it, so ads inside games are
  a safety problem, not just an annoyance.
- Ad scripts also load trackers (Google Tag Manager and others) that report what
  students play to third parties, which conflicts with the privacy rules in
  `AGENTS.md`.
- Removing ads takes revenue from the games' authors. That is a cost to weigh, but
  it does not outweigh showing students ads we can't control.

## What the ads are (sample of 129 game pages, Nov 2026)

Counted by external hosts a game's entry page loads:

| Host | Pages | What it is |
| --- | --- | --- |
| `api.gamemonetize.com` | 41 | the Gamemonetize SDK (interstitial and banner ads) |
| `www.googletagmanager.com` | 27 | tracking |
| `h5gamessdk.yyggames.com` | 8 | an ad SDK |
| `assets.actiongameshub.com` | 2 | an obfuscated ad wrapper (Drive Mad) |
| `pagead2.googlesyndication.com` | 1 | Google AdSense |

Others are links, fonts and libraries. Drive Mad (`/prox/gamemonetize/…`) loads
`rsa.js` and `CocoGA_Gamemonetize.js` from `assets.actiongameshub.com`. Its own game
code does not call into them and already has an "ad blocker present" path, so
removing those two script tags should remove its ads.

## Ways to remove them

1. **Strip ad script tags in the proxy** (`proxy::game_proxy` already rewrites the
   page's HTML): drop `<script src>` tags from a deny-list of ad and tracking hosts.
   Works for games whose own code does not depend on the ad script.
2. **Replace the Gamemonetize SDK with a no-op version** that has the same interface
   (`SDK_OPTIONS.onEvent`, `sdk.showBanner()`) and fires the "ready" and "game
   start" events at once. Some games wait for those events before they start, so
   simply deleting the SDK would leave them stuck.
3. **A content security policy on proxied pages** that only allows scripts from our
   own origin. Blocks everything third party, ads included, but also breaks games
   that load libraries from a CDN.

Each family of games needs testing after a change; this is not done yet.
