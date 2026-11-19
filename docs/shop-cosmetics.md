# Shop cosmetics in chat

Every cosmetic in the shop (name colors, badges, chat effects, profile effects, site themes) is
shown in the Matrix chat client.

- `GET /api/matrix/cosmetics?ids=@a:mitch.pro,...` (`routes/matrix/cosmetics.rs`): signed-in only,
  up to 100 ids. Returns each account's equipped `color`, `badge`, `chatEffect` and `profileEffect`
  from `mitch_lib::shop::public_loadout`, which includes an item only if the account owns it. The
  caller's own site theme comes back as `me.theme`.
- `webserver/matrix/cosmetics.js` finds `[data-user-id]` name elements, batches the lookup, and sets
  `data-mx-color`, `data-mx-effect`, `data-mx-badge` (on the name) and `data-mx-profile` (on the
  message row). It never adds or moves nodes, because the chat client is a React app.
- `webserver/cosmetics.css` styles every item for dark and light mode. Profiles reuse it for the
  items added after the first release (`.display-name.<id>`, `.wrap.<effect>`); the leaderboard has
  its own tables and styles in `leaderboard.js` / `leaderboard.css`.
- Site themes are accent colors (`SHOP_THEMES` in `site-theme.js`) applied from the `shop_theme`
  cookie, which the shop and the chat client set. A custom accent in Preferences wins.

## Adding an item

Add it to `default_shop_catalog` and, if a saved `shop_catalog.json` override may exist, to
`ADDED_DEFAULT_IDS` (the loader appends listed defaults an override lacks). Then add its look to
`cosmetics.css` (and its icon to the badge maps in `matrix/cosmetics.js` and `shop/index.html`).
Cosmetics only: nothing that changes casino odds, balances or limits (`docs/casino.md`).

The Canvas Lock Pass and Quick Access Pass only toggle canvas preferences and have no chat
equivalent. Daily Bonus Plus (+50 coins on the daily login) and Streak Freeze are utilities.
