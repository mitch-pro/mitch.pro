# Search and canonical domains

`mitch.pro` is the canonical domain for the main site and `rjuhsd.school` for the
school pages. `mitchdog.com` is the alternate (see
[blocked-domain-fallback.md](blocked-domain-fallback.md)) and is not used in
canonical tags, `og:url`, sitemaps, or page copy. Code that needs it for sign-in,
passkeys, Matrix, or email stays as it is.

## Per-host files

| File | Served on | Notes |
| --- | --- | --- |
| `webserver/robots.txt`, `webserver/sitemap.xml` | mitch.pro and any other host | URLs must be on mitch.pro |
| `webserver/rjuhsd/robots.txt`, `webserver/rjuhsd/sitemap.xml` | rjuhsd.school | Selected by host in `handler.rs`; test `robots_and_sitemap_are_per_host` |

Each file describes one site, so neither names the other's host. `lastmod` is the
date of the last commit that changed the page; update it when a page's content
changes. Do not disallow scripts or styles a public page needs to render.

## rjuhsd.school

- The root page is Woodcreek's schedule and its canonical; every other school
  has `/<school>/`. `prepare_rjuhsd_html` rewrites the title, description,
  canonical, social tags, hero text, and the first breadcrumb/organization entries
  of the JSON-LD for the requested school (`structured_data_follows_the_school`
  fails if the markup stops matching).
- Copies of the same schedule must not compete with it. `/rjuhsd/` on mitch.pro
  and both `/bell/` pages (mitch.pro and rjuhsd.school) set their canonical to
  `https://rjuhsd.school/` (or the school's path), and stay crawlable so Google
  can read that tag. Do not `Disallow` them.
- `/appeal/`, `/enroll/`, `/preferences/`, and `/api/` are disallowed there.
