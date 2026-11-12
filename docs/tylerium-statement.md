# Tylerium.pro statement page

`webserver/statement/index.html` (`/statement/`) is the owner's public response to the claims in Tyler's Tylerium.pro bio. It is static HTML on the shared legal layout (`/legal.css`), with no backend.

**Not wired up yet.** Nothing links to it, it isn't in `sitemap.xml`, and `/statement` is not in the signed-out exemption list in `handler.rs` (`is_exempt`), so signed-out visitors are redirected. Publishing it means adding it to that list, a footer or banner link, and the sitemap.

## Figures

Lines are counted by commit author name; "Tyler Thompson" and "Codex" are counted as Tyler's, and the table also shows Tyler alone.

- **History** (lines ever added): `git log --numstat --format='AUTHOR:%an'`, summing the added column per author. Total 1,085,498; Tyler + Codex 81,408 (7.5%).
- **Today** (lines in the tree): `git blame -w -M -C --line-porcelain HEAD -- <file>` over tracked files, counting `author` lines. Lockfiles, images, fonts and similar non-code files are excluded. The total is about 636,000; Tyler + Codex is 24,584 (3.9%).

Re-run both on a full clone (`git fetch --unshallow` first) and update the page and this doc together if the numbers are refreshed.
