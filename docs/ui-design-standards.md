# Tylerium design system

Reference: https://tylerium.pro, inspected 6 November 2026 in a real browser at
1440×1000 and 390×844. The public site exposes its sign-in page; authenticated
screens were not inspected. Its public styles also define dashboard and workspace
layouts. Do not claim authenticated behavior was verified from these styles.

## Identity

Use the existing geometric T mark and lowercase `tylerium.` wordmark. The dot is
pale blue. The wordmark and compact metadata use JetBrains Mono; ordinary copy,
forms, and headings use Manrope. Self-host the open-source fonts in production;
never send visitors' requests to a font provider. Keep the upstream font licenses.

The reference uses a midnight-blue background, blue surfaces, pale-blue controls,
thin blue-gray borders, and muted blue-gray secondary text:

| Token | Dark | Light |
| --- | --- | --- |
| Background | #0a101b | #f3f6fa |
| Surface | #151f30 | #ffffff |
| Raised surface | #21314a | #e8eef6 |
| Text | #e0e2e8 | #172536 |
| Secondary text | #b2bfce | #485d73 |
| Border | #52678388 | #52678366 |
| Accent | #9acbfa | #245c8c |
| Text on accent | #061a2d | #ffffff |

The light palette is an accessible companion derived from those colors; it is not
an observed mode of the reference. Define colors once in `web/assets/app.css`.
Use tokens everywhere. Avoid gradients, glass effects, wallpaper, or animation
unless they solve an actual problem. The existing site's mountain wallpaper,
Linux marks, VM labels, and service illustrations are not needed by this app.

## Layout and components

The shared masthead has a maximum width of 1120px, restrained navigation, a compact
brand, and an account menu. Ordinary pages use a 1064px content area with a 24–40px
inset. At narrow widths navigation wraps without horizontal scrolling. Forms use
a 430px column. Sign-in uses a two-column brand/form composition on desktop and a
single column on mobile. Data lists, conversations, game discovery, account forms,
and administration each use a layout suited to their function.

Use 6px control radii and 12px panel radii, 1px borders, 44px minimum touch targets,
and a 4/8px spacing scale. Reserve panels for actual grouping. Do not wrap every
label or statistic in a card. Body copy is 14–16px with 1.6 line-height; page
headings are 30–38px, with normal sentence case and restrained tracking.

All pages load the same CSS and application shell. Include a skip link, landmarks,
visible focus, explicit field labels, live error/status regions, real empty states,
and a working mobile navigation. Respect `prefers-reduced-motion`; no animation
is required. Preserve uploaded avatars in every identity display. Never fetch
third-party avatars, trackers, or game art implicitly.

## Copy and behavior

Use direct labels: Games, Community, People, Messages, Account Settings, Sign In.
Do not fill space with slogans or helper copy. Errors identify the failed action
and a useful next step, without exposing internals. Never invent activity,
statistics, profiles, or testimonials. Every control must have a complete action.

Keep interface and API changes together. Server-side authorization and privacy
rules apply to every caller, including administrators. Test desktop/mobile and
both themes, keyboard access, loading/errors, and all shipped routes. Keep visual
QA artifacts outside the repository. See `AGENTS.md` for mandatory standards.
