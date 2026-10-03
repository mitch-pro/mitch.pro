# mitch.pro

[![Deploy](https://github.com/mitch-pro/mitch.pro/actions/workflows/deploy.yml/badge.svg)](https://github.com/mitch-pro/mitch.pro/actions/workflows/deploy.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

High-performance, self-hosted web platform and student community built in **Rust**. Serves multi-tenant virtual hosting, End-to-End Encrypted (E2EE) chat over a federated Matrix homeserver, Proxmox LXC cloud desktops, real-time WebSockets, and a small arcade of browser games — all from one Axum-based binary with SQLite persistence.

The project started as a Bun/Node server and has since been fully ported to Rust for performance and reliability; `server.js` now only exists as a thin launcher that spawns the compiled `mitch-server` binary.

---

## Architecture & Features

- **Multi-Tenant Domain Routing:** One backend serves distinct front doors — `mitch.pro`, `rjuhsd.school` (a public school bell-schedule hub), and `sexypickleclub.com` — with shared identity and database state.
- **Federated E2EE Chat:** A self-hosted [Conduit](https://gitlab.com/famedly/conduit) Matrix homeserver behind a [Cinny](https://github.com/cinnyapp/cinny) web client, plus a separate lightweight DM system using WebCrypto ECDH P-256 + AES-GCM-256 with timed auto-deletion and encrypted attachment streaming.
- **Hardened Authentication:** Argon2id password hashing, HttpOnly session cookies, email/username login resolution, and same-origin CSRF protection on every mutating API call.
- **Cloud Desktops:** Automated, ephemeral Proxmox LXC container lifecycle management with an in-browser VNC/SSH terminal gateway and idle auto-cleanup.
- **Real-Time WebSockets:** Live pixel canvas, multiplayer chess, presence/online indicators, and push notifications.
- **SQLite + Flat-File Persistence:** Crash-resilient SQLite (WAL mode) alongside version-controlled JSON configuration in `data/`.

---

## Directory Structure

```text
mitch.pro/
├── rust/                    # The actual server — an Axum/Tokio Cargo workspace
│   ├── crates/
│   │   ├── mitch-server/    # Main HTTP/WebSocket server: routing, auth, static serving
│   │   ├── mitch-lib/       # Shared domain logic (ported 1:1 from the original JS)
│   │   ├── mitch-mail/      # Outbound/inbound mail pipeline
│   │   └── mitch-ssh-gateway/ # Isolated SSH gateway for admin container access
│   └── Dockerfile
├── webserver/                # Static assets, page templates, and client-side JS/CSS
├── data/                      # SQLite database + version-controlled JSON configs
├── caddy/                     # Reverse proxy config for the blue/green deploy
├── mail/                      # Legacy JS mail scripts (nodemailer fallback)
├── lib/                       # Legacy JS modules kept as test fixtures/oracles
├── tests/                     # Rust and JS test suites
├── tools/                     # Admin, maintenance, and asset-generation scripts
├── docs/                      # Design notes for individual features
├── webvm/                     # In-browser x86 WebVM integration
├── docker-compose.yml         # Full service topology (server, Matrix, mail, SSH, proxies)
└── server.js                  # Thin launcher — spawns the compiled Rust binary
```

---

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs) (stable toolchain) — the server itself
- [Bun](https://bun.sh) (v1.2+) — test runner and a few maintenance scripts
- Docker & Docker Compose (optional, for the full containerized stack)

### Running Locally

1. **Build and run the server:**
   ```bash
   cargo run --release --manifest-path rust/Cargo.toml -p mitch-server
   ```
   or, equivalently, via the launcher script:
   ```bash
   bun server.js
   ```
   The application listens on `http://0.0.0.0:6800` by default.

2. **Configure environment:**
   ```bash
   cp .env.example .env
   # Edit .env with your environment secrets, or load via Doppler
   ```

### Running with Docker

```bash
docker compose up --build
```

This brings up the full topology: the Rust server (blue/green behind Caddy), the Conduit Matrix homeserver, LiveKit, the Rust mail pipeline, and the SSH gateway.

---

## Configuration & Environment

Key environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `NODE_ENV` | Runtime environment (`production` or `development`) | `development` |
| `SESSION_COOKIE_SECURE` | Enforce `Secure` attribute on session cookies | `0` (`1` in production) |
| `SMS_WEBHOOK_SECRET` | Authentication secret for inbound SMS webhook | None |
| `SSH_GATEWAY_URL` | WebSocket URI for the SSH gateway | `ws://ssh-gateway:6820` |
| `ENABLE_ADMIN_KEY_HEADER` | Break-glass admin bypass header (keep disabled) | `0` |
| `NTFY_TOPIC` / `NTFY_USER` / `NTFY_PASS` | Optional [ntfy](https://ntfy.sh) push notification credentials | None |

---

## Testing

```bash
cargo test --manifest-path rust/Cargo.toml --workspace   # Rust unit tests (primary suite)
bun run test:unit                                         # Same, via the npm script alias
bun run test:integration                                  # Full JS integration suite (needs a running server)
bun test                                                   # Everything
```

See [tests/README.md](tests/README.md) for the full breakdown, including the legacy JS unit tests kept around for coverage the Rust port hasn't fully absorbed yet.

---

## Security

Please report vulnerabilities privately. See [SECURITY.md](SECURITY.md) for the reporting policy and [SECURITY_POSTURE.md](SECURITY_POSTURE.md) for an overview of defensive controls.

---

## Third-Party & Open Source Credits

- **[Conduit](https://gitlab.com/famedly/conduit)** — the Matrix homeserver powering chat (Apache-2.0).
- **[Cinny](https://github.com/cinnyapp/cinny)** — the Matrix web client at `webserver/matrix/` (AGPL-3.0; see [`webserver/matrix/LICENSE`](webserver/matrix/LICENSE)).

---

## License

Apache License 2.0 — see [LICENSE](LICENSE) for the full text. The license
covers the code only; the mitch.pro name and branding are not covered — see
[TRADEMARK.md](TRADEMARK.md).
