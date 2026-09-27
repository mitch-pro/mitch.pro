# mitch.pro / bun-server

High-performance, self-hosted web platform and student portal built on [Bun](https://bun.sh). Provides multi-tenant virtual hosting, End-to-End Encrypted (E2EE) Secure Chat, Proxmox LXC container provisioning, WebSockets, and interactive browser applications with SQLite WAL persistence.

---

## Architecture & Features

- **Multi-Tenant Domain Routing:** Serves distinct web front doors (`mitch.pro`, `rjuhsd.school`, and `sexypickleclub.com`) from a unified backend with shared identity and database state.
- **End-to-End Encrypted Chat:** WebCrypto ECDH P-256 key agreement + AES-GCM-256 encrypted direct and group messaging with timed auto-deletion (1-hour/custom post-read pruning) and secure attachment streaming (250MB user quota).
- **Hardened Authentication:** Argon2id password hashing, HttpOnly session tokens (`mitch_session`), email/username login resolution, and same-origin CSRF protection (`X-Mitch-Requested-With`).
- **Proxmox Cloud & Container Orchestration:** Automated ephemeral Linux container (LXC) lifecycle management, VNC/SSH terminal gateway, and automated cleanup routines.
- **SQLite WAL & Preserved Configurations:** Fast, crash-resilient SQLite storage (`data/mitchpro.db`) operating alongside version-controlled flat configurations in `data/`.
- **Real-Time WebSockets:** Live pixel canvas, multiplayer chess, chat presence, notification broadcasting, and push alerts.

---

## Directory Structure

```text
bun-server/
├── caddy/             # Caddy reverse proxy configuration and blue/green deploy setup
├── data/              # Runtime SQLite database (mitchpro.db), games libraries, and configs
├── lib/               # Shared backend libraries (SQLite data store, DB migrations)
├── logs/              # Structured runtime and service logs
├── mail/              # SMTP outbound notification and email dispatch scripts
├── models/            # Domain models and application schemas
├── proxy/             # Dedicated reverse proxies and stream helpers
├── routes/             # Modular route definitions and API controllers
├── ssh-gateway/       # Secure SSH gateway service for container access
├── tests/             # Unit and end-to-end integration test suites
├── tools/             # Administrative maintenance, scraping, and inspection utilities
├── webserver/         # Static web assets, SPA frontends, games, and UI components
├── webvm/             # Virtual x86 WebVM integration and disk image assets
├── server.js          # Main Bun HTTP/WebSocket server and API router
├── Dockerfile         # Production container build
└── docker-compose.yml # Container orchestration and service definitions
```

---

## Quick Start

### Prerequisites

- [Bun](https://bun.sh) (v1.2+)
- Docker & Docker Compose (optional for containerized setup)

### Running Locally

1. **Install dependencies:**
   ```bash
   bun install
   ```

2. **Configure environment:**
   ```bash
   cp .env.example .env
   # Edit .env with your environment secrets or load via Doppler
   ```

3. **Start the development server:**
   ```bash
   bun server.js
   ```
   The application listens on `http://0.0.0.0:6800`.

### Running with Docker

```bash
docker compose up --build
```

---

## Configuration & Environment

Key environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `NODE_ENV` | Runtime environment (`production` or `development`) | `development` |
| `SESSION_COOKIE_SECURE` | Enforce `Secure` attribute on session cookies | `0` (`1` in production) |
| `SMS_WEBHOOK_SECRET` | Authentication secret for inbound SMS webhook | None |
| `SSH_GATEWAY_URL` | WebSocket URI for SSH gateway | `ws://ssh-gateway:6820` |
| `ENABLE_ADMIN_KEY_HEADER`| Break-glass admin bypass header (keep disabled) | `0` |
| `NTFY_TOPIC` | Optional push notification topic | None |
| `NTFY_USER` | Optional ntfy basic auth username | None |
| `NTFY_PASS` | Optional ntfy basic auth password | None |

---

## Testing

Run the test suites:

```bash
bun run test:unit                  # Fast unit tests (no running server required)
bun run test:integration           # Full master integration suite (requires running server)
bun test                           # All test suites
```

See [tests/README.md](tests/README.md) for more details.

---

## Security

Please report vulnerabilities privately. See [SECURITY.md](SECURITY.md) for reporting policy and [SECURITY_POSTURE.md](SECURITY_POSTURE.md) for an overview of defensive controls.

---

## Third-Party & Open Source Credits

This project integrates open-source software to power its federated Matrix chat infrastructure:
- **[Conduit](https://gitlab.com/famedly/conduit)**: High-performance Matrix homeserver written in Rust (licensed under Apache-2.0).
- **[Cinny](https://github.com/cinnyapp/cinny)**: Matrix web client frontend located at `webserver/matrix/` (licensed under AGPL-3.0; see [`webserver/matrix/LICENSE`](webserver/matrix/LICENSE) for the full license text and copyright notices).

---

## License

This project is licensed under the Apache License 2.0 with Commons Clause restriction — see [LICENSE](LICENSE) for details.
