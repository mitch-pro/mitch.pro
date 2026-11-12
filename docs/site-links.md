# Homepage page list (`site_links`)

The homepage's "All pages" list is the `site_links` SQL table, managed by
`mitch_lib::site_links`. It replaces the plain-text `data/sites` file, which
was hand-edited, regex-patched by the admin mirror tool and re-parsed by three
readers. `data/sites` is no longer in the repository or read at runtime.

| Column | Meaning |
| --- | --- |
| `kind` | How the homepage opens it: `url`, `iframe`, `popup`, ... |
| `href`, `label` | Destination and display name. Local pages start with `/`. |
| `admin_only` | Shown only to admins (the old `admin ` prefix). |
| `visible` | `0` hides the link without deleting it (the old `//` lines). |
| `deprecated` | The page was retired from navigation. Always hidden. |
| `sort_order` | Display order. The homepage shows one flat grid and uses `categoryForLabel` in `index.html` only to order links (no visible category headings). |

## Startup behavior (`site_links::seed`)

1. If the table is empty, import `data/sites` when it still exists (production
   keeps its admin edits), otherwise the built-in `DEFAULT_LINKS`.
2. Every start: rows whose `href` is in `RETIRED_HREFS` are forced to
   `visible = 0, deprecated = 1`, so an old database or old file cannot bring a
   retired link back. Add to that list to retire another page.
3. Every start: required links (currently `/learn/` and `/support/`, which fill the homepage's 3×5 grid) are inserted if missing.

## Readers

- `GET /api/pass` (homepage) returns the visible rows in the legacy line format
  (`[admin ]kind href label`) that `parseSites()` reads. Admin rows are included
  only for admins.
- `alias.rs` hands out URL-alias section UUIDs for the visible local links
  (`docs/url-aliases.md`).
- The admin content-mirror tool (`/api/admin/content/mirror`) updates the
  "Mitch.pro Mirrors" row through `site_links::set_mirror_url`.

There is no admin page for editing the list; change rows with SQL or add a
seed/retirement entry in code.

See `docs/deprecated-pages.md` for what was retired.
