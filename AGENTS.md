# Tylerium.pro project rules

Read this file before making any change. These rules apply to coding agents,
contributors, reviews, and future modifications. Follow the design system in
`docs/ui-design-standards.md`; update both documents when legitimate conventions
change. User instructions take precedence, and credentials never imply permission.

## Design and interface writing

- Use https://tylerium.pro as the primary visual reference. Its midnight-blue
  palette, pale-blue accent, Manrope body type, JetBrains Mono wordmark and
  compact metadata, restrained borders, and measured spacing define Tylerium.
- Every page must use the shared shell and design tokens. Keep typography,
  weights, spacing, controls, validation, loading, empty states, notifications,
  navigation, and focus states consistent. Pages may use different layouts when
  their functions differ. Reuse existing components and project conventions.
- Do not introduce generic SaaS landing pages, repetitive rounded-card grids,
  oversized slogan heroes, unnecessary gradients, neon, glow, glassmorphism,
  decorative blur, emojis, stock illustrations, or gratuitous motion. A visual
  treatment is justified only by Tylerium's established identity and usability;
  looking modern is not a reason. Every visual element must serve a purpose.
- Write natural, concise, specific interface copy. Review every new heading,
  paragraph, button, tooltip, notification, and error message. Remove text that
  provides no useful information. Prefer labels such as Account Settings,
  Projects, Sign In, and Recent Activity when they describe the actual content.
- No corporate buzzwords, exaggerated promises, filler paragraphs, meaningless
  slogans, artificial enthusiasm, excessive adjectives or exclamation marks,
  vague feature descriptions, or repetitive sentence structures.
- Prohibited wording includes “Make it yours,” “Unleash your potential,”
  “Unlock the power of,” “Elevate your experience,” “Take it to the next level,”
  “Experience the future,” “Seamless experience,” “Empower your journey,”
  “Built for creators,” “Transform the way you,” “Your journey starts here,”
  “Discover endless possibilities,” “Reimagine what's possible,” “Designed
  with you in mind,” and “One platform, endless possibilities.” Similar generic
  marketing language is prohibited even when not listed here.
- No dead buttons, fake statistics, invented testimonials, decorative charts
  with fabricated data, mock production users, or placeholder projects presented
  as real. Implement a feature completely, remove it, or state its actual status.
- Keep code maintainable, secure, efficient, and consistent with the project.
  Avoid unnecessary dependencies, UI libraries, duplicate components, unfinished
  implementations, hardcoded credentials, and excessive abstraction. Add features
  only when they contribute to the actual site.
- Test before declaring completion. Check desktop and mobile, keyboard access,
  contrast, reduced motion, natural copy, and shared visual consistency. Record
  unverified behavior honestly; a plan or passing build is not proof of behavior.

## Fresh deployment and retired features

- Tylerium uses a fresh database, credentials, sessions, and upload directory.
  Never copy existing accounts, profiles, messages, uploads, activity, or other
  personal records into the new deployment. Never seed fake production records.
- Preserve existing production data and backups. A clean deployment is separate
  storage, never an automatic reset, import, or destructive migration.
- Remove all website VM/LXC/lab/provisioning/console routes, permissions, workers,
  models, integrations, and dependencies. Do not operate, delete, stop, or alter
  real virtual machines, hypervisors, Proxmox, or their configurations.
- Do not send legacy email during development, tests, or deployment. Remove
  obsolete mail/SMTP/newsletter integrations. Document account-recovery behavior.
- Rebrand deliberately, inspecting references and behavior instead of applying
  blind replacements. Retire obsolete integrations and unrelated domain routing.
- Use a separate least-privilege service and storage. Prepare a reviewable change
  before asking for approval to replace the live site, change DNS, or perform an
  irreversible action. Do not change unrelated services or infrastructure.

## Retained application and economy

- Preserve all non-VM features, including the economy, shop, casino, cosmetics,
  community, messaging, games, and needed administration. Do not silently replace
  the application with a smaller demo. Use Coins as the interface currency label.
- Every balance, catalog, refund, and payout change requires checking for risk-free
  or unbounded profit. Remove retired effects at their enforcement point and from
  catalog overrides, including effects already equipped on existing accounts.
  Use one catalog loader, never independent administrator copies.
- Keep every backend feature wired to its working frontend in the same change.
  Preserve chosen avatars and equipped cosmetics in every identity display.

## Engineering, privacy, and verification

- New persistent data belongs in proper SQL columns/tables with lookup indexes.
  No new JSON-blob documents or new flat-file JSON storage. Update subsystem docs
  in the same change; explain non-obvious invariants and security assumptions.
- Never hardcode secrets, passwords, private contact addresses, or API keys. Use
  environment configuration or secure operator inputs. Never log secret values.
- Never track, aggregate, or expose browsing locations, current tabs, or user
  actions to staff or other users without explicit opt-in. Presence stays in
  memory and must not be logged or stored. Privacy settings apply server-side to
  friends, administrators, and every other API consumer.
- Use port 8080 for local testing when free. Do not stop unrelated processes.
  Test authentication with isolated temporary storage, never production identities.
- Run `cargo test --workspace`, `cargo fmt --all -- --check`, relevant frontend
  checks, and critical integration tests before committing. Commit completed work
  in this repository, and check CI after each commit. Do not push a major
  integration to master or replace production without explicit approval.
- Keep test artifacts, generated media, personal records, uploads, credentials,
  databases, and temporary fixtures out of Git. Keep licensing and attribution.
- Production inspection and preparation are authorized by the rebuild request.
  Do not alter real VMs, unrelated services, or existing data. Ask for approval
  before live replacement, DNS changes, irreversible operations, or deletion of
  existing data/backups. Keep deployment changes concrete and reviewable.
