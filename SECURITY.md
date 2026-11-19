# Security Policy

mitch.pro is a live platform used by students — security reports are taken
seriously and triaged promptly. This document covers **how to report a
vulnerability**. For a technical summary of the controls already in place and
an operator hardening checklist, see [SECURITY_POSTURE.md](SECURITY_POSTURE.md).
For data-retention and privacy guarantees, see [docs/privacy.md](docs/privacy.md).

## Supported Versions

Only the code currently deployed from `master` is supported. There are no
maintained release branches or backported fixes — if you find an issue on
`master`, assume it affects production unless confirmed otherwise.

## Reporting a Vulnerability

**Do not open a public GitHub issue for anything exploitable.** Public issues
are appropriate for typos or non-security bugs; anything that could compromise
an account, expose another user's data, bypass access controls, or disrupt the
service should be reported privately through one of these channels:

1. **[GitHub Security Advisories](https://github.com/mitch-pro/mitch.pro/security/advisories/new)** (preferred) —
   private by default, supports attaching proof-of-concept files, and keeps a
   timestamped record.
2. **Email:** [mitch@mitch.pro](mailto:mitch@mitch.pro) — if you want to
   encrypt sensitive details, ask for a PGP key in your first message.

### What to Include

Reports are triaged fastest when they include:

- **Impact** — what an attacker can accomplish (account takeover, reading
  another user's data, privilege escalation, secret disclosure, etc.)
- **Steps to reproduce** — concrete enough to replay directly
- **Affected path(s)** — route, API endpoint, or page
- **Proof of concept** — a minimal request or script if available. **Do not**
  run anything against production that affects real student accounts or data
  beyond what is strictly necessary to demonstrate the issue (see Scope below).

### Response Targets

| Stage | Target |
|---|---|
| Acknowledgment | Within 48 hours |
| Initial triage / severity assessment | Within 5 business days |
| Fix for critical/high severity | Prioritized immediately, best-effort |
| Public disclosure | Coordinated with the reporter once a fix is deployed |

This is a small, independently maintained project — these are good-faith
targets, not a contractual SLA.

## Scope

**In scope:** the live deployments at `mitch.pro`, `mitchdog.com`,
`rjuhsd.school`, and `sexypickleclub.com`, along with all code in this
repository.

**Out of scope / please don't:**

- Automated vulnerability scanners or load/stress testing against production
  without prior coordination
- Social engineering of staff, moderators, or students
- Physical attacks against infrastructure
- Spam, content moderation, or abuse reports (use the in-app **Report** feature
  or [Feedback](https://mitch.pro/feedback/) instead)
- Vulnerabilities in third-party software or services this project depends on
  but does not control (report those upstream — e.g., Conduit, Cinny, LiveKit,
  Proxmox, noVNC, Discord)
- Denial-of-service attacks of any kind
- Accessing, modifying, or exfiltrating another user's data beyond the minimum
  needed to prove a vulnerability exists — demonstrate impact against your own
  test account whenever possible

## Safe Harbor

Good-faith security research conducted in line with this policy — within scope,
without harming real users or degrading the service, and reported promptly and
privately — will not result in legal action from this project. If a third party
initiates legal action related to research conducted in good-faith compliance
with this policy, we will make it known that your actions were authorized.

This safe harbor does not extend to third-party systems (see Scope).

## Disclosure Policy

Please allow a reasonable window to ship a fix before any public write-up or
disclosure so students using the platform remain protected. We will keep you
updated on remediation progress and are glad to credit you (by name, handle, or
anonymously) once the issue is resolved, unless you prefer not to be credited.

There is no paid bug bounty program at this time.

## Production Hardening & Automated Checks

- If you are self-hosting this repository, review the **Operator checklist** in
  [SECURITY_POSTURE.md](SECURITY_POSTURE.md#operator-checklist-before-production)
  before going live.
- Pull requests against `master` are also checked by automated static screening
  ([`docs/ci-pr-screening.md`](docs/ci-pr-screening.md)) and an email-leak
  scanner ([`docs/email-check.md`](docs/email-check.md)).
