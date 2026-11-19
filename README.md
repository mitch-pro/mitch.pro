# mitch.pro

[![CI](https://github.com/mitch-pro/mitch.pro/actions/workflows/rust-ci.yml/badge.svg)](https://github.com/mitch-pro/mitch.pro/actions/workflows/rust-ci.yml)
[![Deploy](https://github.com/mitch-pro/mitch.pro/actions/workflows/deploy.yml/badge.svg)](https://github.com/mitch-pro/mitch.pro/actions/workflows/deploy.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

Self-hosted web platform and student community written in **Rust**. One Axum/Tokio server handles multi-tenant virtual hosting, Matrix chat integration, Proxmox cloud computers, real-time WebSockets, and a browser game portal backed by SQLite.

---

## Architecture & Features

- **Multi-Tenant Domain Routing:** One server serves distinct front doors — `mitch.pro` / `mitchdog.com`, `rjuhsd.school` (a school bell-schedule hub, plus configurable mirror domains), and `sexypickleclub.com` — with shared database state and per-domain isolation.
- **Matrix Chat & Voice/Video:** Self-hosted, non-federated [Conduit](https://gitlab.com/famedly/conduit) Matrix homeserver paired with a [Cinny](https://github.com/cinnyapp/cinny) web client, client-side E2EE key backup/recovery (`docs/matrix-e2ee.md`), and a [LiveKit](https://livekit.io) SFU for Matrix voice and video calls.
- **Authentication & Access Control:** Argon2id password hashing, HttpOnly `mitch_session` cookies (stored as SHA-256 hashes in SQLite), WebAuthn passkeys, TOTP 2FA, magic-link login, same-origin CSRF enforcement (`Origin`/`Referer` + `X-Mitch-Requested-With: 1`), and an Argon2id admin passphrase gate on `/api/admin/*`.
- **Cloud Computers (Proxmox):** Proxmox VM and LXC provisioning and lifecycle management with an in-browser [noVNC](https://novnc.com) graphical console proxied over `/api/vm/desktop/ws`, single-use console handles, and owner-consented admin access (`docs/computer-portal.md`, `docs/vm-access.md`).
- **Real-Time WebSockets & Arcade:** Authenticated `/ws` broadcast sockets for ephemeral in-memory presence, live notifications, pixel canvas, multiplayer games (chess, battleship, jeopardy), and a browser game portal with encrypted cloud save sync.
- **Persistence & Encryption at Rest:** SQLite in WAL mode (with optional Litestream tmpfs RAM-disk replication via `sql-sync/`), seed catalogs in `data/`, and AES-256-GCM encryption at rest for user save files (`docs/encryption-at-rest.md`).

---

## Directory Structure

```text
mitch.pro/
├── rust/                    # Cargo workspace (Axum/Tokio)
│   ├── crates/
│   │   ├── mitch-server/    # HTTP/WebSocket server: routing, auth, APIs, static serving
│   │   ├── mitch-lib/       # Shared domain logic, SQLite storage, auth, and crypto
│   │   └── mitch-mail/      # Outbound SMTP and inbound IMAP mail service
│   └── Dockerfile
├── webserver/               # Static HTML/CSS/JS pages, Cinny (matrix/), and noVNC (novnc/)
├── data/                    # Runtime SQLite DB (mitchpro.db), seed catalogs, and example configs
├── caddy/                   # Reverse proxy config for blue/green deploys and security headers
├── sql-sync/                # Litestream sidecar for optional RAM-disk SQLite replication
├── egress-proxy/            # Host-network CONNECT proxy binding movie streams to the VPN interface
├── mail/                    # Standalone Python Gmail helper and watcher scripts
├── admin/                   # Operator CLI scripts for inspection and maintenance
├── tools/                   # Deployment (deploy.sh), firewall, template, and maintenance scripts
├── tests/                   # Python, Node.js, and shell test suites (Rust tests live in rust/crates/)
├── docs/                    # Architecture, security, privacy, and feature documentation
├── infra/                   # Cloudflare DNS automation
└── docker-compose.yml       # Full stack (Caddy, blue/green server, sql-sync, mail, Conduit, LiveKit, egress)
```

---

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs) (stable toolchain)
- Python 3.9+ (for scripts in `tools/` and Python tests in `tests/`)
- Node.js 20+ (optional, for the JavaScript unit tests in `tests/`)
- Docker & Docker Compose (optional, for the full containerized stack)

### Running Locally

1. **Configure environment (optional for local development):**
   ```bash
   cp .env.example .env
   ```
   Edit `.env` with your local settings, or supply secrets via Infisical (`docs/secrets-providers.md`).

2. **Build and run the server:**
   ```bash
   cargo run --manifest-path rust/Cargo.toml -p mitch-server
   ```
   The server binds to `0.0.0.0:6801` if `PORT` is unset (`.env.example` and Docker set `PORT=6800`).

   For local development without creating an account, run on port `8080` with `GOD_MODE=1`:
   ```bash
   PORT=8080 GOD_MODE=1 cargo run --manifest-path rust/Cargo.toml -p mitch-server
   ```
   `GOD_MODE=1` automatically signs every visitor in as the local dev admin (`admin@mitch.pro`). It is refused when `NODE_ENV=production` and must never be set on a deployed server.

### Running with Docker

```bash
docker compose up --build
```

This starts the full service stack: Caddy (`reverse-proxy` on port `6800`), blue/green `mitch-server` slots, `sql-sync` (Litestream), `mail-rs` (`mitch-mail`), Conduit (Matrix), LiveKit (voice/video SFU), and `movies-egress`.

---

## Configuration & Environment

See [`.env.example`](.env.example) for the complete template and [`docs/secrets-providers.md`](docs/secrets-providers.md) for how production secrets are injected with Infisical.

| Variable | Description | Default |
|----------|-------------|---------|
| `PORT` | HTTP listen port for `mitch-server` | `6801` (`6800` in `.env.example` / Docker) |
| `NODE_ENV` | Runtime mode (`production`, `development`, or `test`) | `development` |
| `SESSION_COOKIE_SECURE` | Set `Secure` attribute on `mitch_session` cookies | `0` (`1` in production) |
| `DATA_DIR` | Directory for SQLite data, saves, and config files | `./data` |
| `MITCH_DB_PATH` | Override path to `mitchpro.db` (e.g., `/ramdisk/mitchpro.db`) | `<DATA_DIR>/mitchpro.db` |
| `DATA_ENCRYPTION_KEY` | 32-byte master key (hex/base64) for encrypted user saves | Auto-generated in `data/encryption.key` |
| `SECRET_KEY` | Signing secret for server tokens | None |
| `GOD_MODE` | Local-dev-only auto-login as dev admin (ignored in production) | `0` |
| `PROXMOX_HOST` / `PROXMOX_TOKEN_ID` / `PROXMOX_TOKEN_SECRET` | Proxmox API connection and scoped token for cloud desktops | None |
| `NTFY_TOPIC` / `NTFY_USER` / `NTFY_PASS` | Optional [ntfy](https://ntfy.sh) operator notifications | None |

---

## Testing & Validation

CI (`.github/workflows/rust-ci.yml`) runs formatting, Clippy, and the full Rust test suite on every push and pull request:

```bash
cargo fmt --manifest-path rust/Cargo.toml --all -- --check
cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path rust/Cargo.toml --workspace
```

See [`tests/README.md`](tests/README.md) for details on the Rust integration tests, Python maintenance-script tests, Node.js helper tests, and shell test suites.

---

## Documentation

- **Security:** [`SECURITY.md`](SECURITY.md) (vulnerability reporting) · [`SECURITY_POSTURE.md`](SECURITY_POSTURE.md) (technical controls & production checklist)
- **Privacy:** [`docs/privacy.md`](docs/privacy.md) (data minimization & retention)
- **Storage & Secrets:** [`docs/encryption-at-rest.md`](docs/encryption-at-rest.md) · [`docs/database-ramdisk.md`](docs/database-ramdisk.md) · [`docs/secrets-providers.md`](docs/secrets-providers.md)
- **Administration & Infrastructure:** [`docs/admin-panel.md`](docs/admin-panel.md) · [`docs/computer-portal.md`](docs/computer-portal.md) · [`docs/vm-access.md`](docs/vm-access.md) · [`docs/deploy-health-check.md`](docs/deploy-health-check.md)
- **Frontend Standards:** [`docs/ui-design-standards.md`](docs/ui-design-standards.md)

---

## Third-Party & Open Source Credits

- **[Conduit](https://gitlab.com/famedly/conduit)** — Matrix homeserver powering chat (Apache-2.0).
- **[Cinny](https://github.com/cinnyapp/cinny)** — Matrix web client in `webserver/matrix/` (AGPL-3.0; see [`webserver/matrix/LICENSE`](webserver/matrix/LICENSE)).
- **[LiveKit](https://livekit.io)** — WebRTC SFU for Matrix voice and video calls (Apache-2.0).
- **[noVNC](https://novnc.com)** — In-browser VNC client in `webserver/novnc/` (MPL-2.0).
- **[Litestream](https://litestream.io)** — Streaming SQLite replication sidecar in `sql-sync/` (Apache-2.0).
- **[Caddy](https://caddyserver.com)** — Reverse proxy for blue/green deployments (Apache-2.0).

---

## License

Apache License 2.0 — see [`LICENSE`](LICENSE) for the full text. The license covers the code only; the mitch.pro name and branding are not covered — see [`TRADEMARK.md`](TRADEMARK.md).
