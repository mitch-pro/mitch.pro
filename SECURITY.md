# Security Policy

mitch.pro is a real, live platform used by real students — security reports are
taken seriously and triaged quickly. This document covers the **process** for
reporting a vulnerability. For a technical breakdown of the controls already in
place, see [SECURITY_POSTURE.md](SECURITY_POSTURE.md).

## Supported Versions

Only the code currently deployed from `master` is supported. There are no
maintained release branches or back-ported fixes — if you find an issue,
assume it affects production until told otherwise.

## Reporting a Vulnerability

**Please do not open a public GitHub issue for anything exploitable.** Public
issues are fine for things like typos or non-security bugs; anything that
could be used to compromise an account, read someone else's data, or disrupt
the service should be reported privately through one of these channels:

1. **[GitHub Security Advisories](../../security/advisories/new)** (preferred) —
   private by default, lets you attach files/PoCs, and keeps a timestamped
   record for both of us.
2. **Email:** [mitch@mitch.pro](mailto:mitch@mitch.pro) — if you'd like to
   encrypt anything sensitive, ask for a PGP key in your first message.

### What to include

A report is actionable fastest when it has:

- **Impact** — what a real attacker could actually do with it (read another
  user's messages, bypass auth, escalate privilege, exfiltrate secrets, etc.)
- **Steps to reproduce** — concrete enough that it can be replayed exactly
- **Affected path(s)** — route, endpoint, or page
- A proof-of-concept request/script if one exists. **Do not** run anything
  against production that could affect real student accounts or data beyond
  what's strictly necessary to demonstrate the issue — see Scope below.

### Response expectations

| Stage | Target |
|---|---|
| Acknowledgment | Within 48 hours |
| Initial triage / severity assessment | Within 5 business days |
| Fix for critical/high severity | Best-effort, prioritized immediately |
| Public disclosure | Coordinated with the reporter once a fix is deployed |

This is a small, independently run project — these are good-faith targets,
not a contractual SLA.

## Scope

**In scope:** the live deployments at `mitch.pro`, `rjuhsd.school`,
`sexypickleclub.com`, and `mitchdog.com`, and any code in this repository.

**Out of scope / please don't:**

- Automated vulnerability scanners or load/stress testing against production
  without prior coordination — this is a real service students rely on daily
- Social engineering of staff, moderators, or students
- Physical access attacks against infrastructure
- Spam, content moderation, or abuse reports (these aren't security issues —
  use the in-app **Report** feature or [Feedback](https://mitch.pro/feedback/) instead)
- Vulnerabilities in third-party services this project depends on but doesn't
  control (report those upstream — e.g. Conduit, Cinny, Proxmox, Discord)
- Denial-of-service attacks of any kind
- Accessing, modifying, or exfiltrating another real user's data beyond the
  minimum needed to prove a vulnerability exists — demonstrate impact against
  your own test account wherever possible

## Safe Harbor

Good-faith security research conducted in line with this policy — within
scope, without harming real users or degrading the service, and reported
promptly and privately — will not result in legal action from this project.
If a third party initiates legal action related to research you've conducted
in good-faith compliance with this policy, we will make it known that your
actions were authorized.

This safe harbor does not extend to third-party systems (see Scope).

## Disclosure Policy

We ask that you give us a reasonable window to ship a fix before any public
write-up or disclosure — coordinated disclosure protects the students using
this platform. We'll keep you updated on remediation progress and are happy
to credit you (by name, handle, or anonymously, your choice) once the issue
is resolved, unless you'd rather stay out of it entirely.

There is no paid bug bounty program at this time.

## Production Hardening Expectations

If you're standing up your own instance of this code, see the **Operator
checklist** in [SECURITY_POSTURE.md](SECURITY_POSTURE.md#operator-checklist-before-production)
before going live.
