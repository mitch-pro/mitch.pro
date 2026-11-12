# Secrets providers

Secrets (API keys, `DATA_ENCRYPTION_KEY`, ntfy credentials) reach the containers
as environment variables, injected by the host when it starts them. The app never
talks to a secrets manager. Two providers are supported, chosen on the host:

| Provider | Selected by | Injects with |
| --- | --- | --- |
| Doppler | default | `doppler run -- docker compose ...` |
| Infisical | `INFISICAL=True` in the repo's `.env` | `infisical run -- docker compose ...` |

The choice is made by a small block of shell functions (`secrets_init`,
`secrets_get`, `secrets_run`, ...) that is copied into `tools/deploy.sh`,
`newsletter.sh`, `tools/update_ip.sh` and `tools/add_cloudflare_domain.sh`, so one
flag moves them all. It is copied rather than sourced from a shared file on purpose:
these scripts run as root, and a helper read in from the repo checkout would be one
more file whose modification gives root. `tests/test_secrets_provider.sh` fails if
the copies differ. Edit all four together (the block sits between `>>> secrets-provider`
and `<<< secrets-provider` markers). With the flag unset, behaviour is the same as before
Infisical support was added.

## Configuring Infisical

Everything below is done by hand on the server, as root. Only the `INFISICAL=True`
flag goes in the project's `.env`; the domain and token never go in the repo.

### How the scripts call it

`deploy.sh` runs as root (`sudo deploy.sh`, installed at `/usr/local/bin/deploy.sh`)
and calls `infisical` directly, not through sudo:

- `infisical export --format=json` to check availability (`secrets_available`)
- `infisical secrets get NAME --plain` to read a single value
- `infisical run -- docker compose ...` to start the containers with secrets injected

### Install the CLI

On Debian/Ubuntu, as root:

```sh
curl -1sLf 'https://artifacts-cli.infisical.com/setup.deb.sh' | bash
apt-get update && apt-get install -y infisical
```

`infisical-core` is only the server image, not the CLI.

### Auth: service token and self-hosted domain, root-only

The CLI defaults to US cloud, so the domain must be set for every command, using the
`INFISICAL_DOMAIN` environment variable. It takes precedence over `domain` in
`.infisical.json`.

sudo strips the caller's environment, so both values live in a root-only file,
`/etc/infisical/env` (directory mode 700, file mode 600, owner `root:root`), one
`VAR=value` per line:

```
INFISICAL_DOMAIN=https://infisical.example.com
INFISICAL_TOKEN=st.xxxx
```

Create it without leaving the token in shell history:

```sh
install -d -m 700 /etc/infisical
bash -c 'umask 077; read -rsp "Token: " t; echo
  printf "INFISICAL_DOMAIN=https://infisical.example.com\nINFISICAL_TOKEN=%s\n" "$t" > /etc/infisical/env'
```

sudo loads that file into the environment of `deploy.sh` only, through a sudoers
drop-in (edit with `visudo -f /etc/sudoers.d/infisical`):

```
Defaults!/usr/local/bin/deploy.sh env_file=/etc/infisical/env
```

The path must match the real path of the script that sudo runs. Everything `deploy.sh`
launches (`infisical export`, `secrets get`, `run`) inherits the variables.

The `mitch` user only has sudo for `sudo deploy.sh`. It can't read `/etc/infisical/env`
or change the domain or token; editing or deleting the file needs root.

Running as root directly (`ssh root@host`), the `env_file` rule does not apply. Load the
file by hand first:

```sh
set -a; . /etc/infisical/env; set +a
```

Service tokens are deprecated upstream in favor of machine identities with Universal
Auth. Moving to one is a future improvement.

### Verify

```sh
set -a; . /etc/infisical/env; set +a
cd /home/mitch/server/bun
infisical export --format=json | head -c 100
```

This is exactly what `secrets_available` runs, and it must succeed, or `deploy.sh`
silently uses the `.env` fallback. `infisical export` defaults to `--env=dev`, so check
that the token's environment matches the one you want. How the token's scope interacts
with that default hasn't been confirmed, so treat it as something to check, not as
fact.

### Switching

Copy the secrets across in Infisical, add `INFISICAL=True` to `.env`, and deploy. If
`infisical export` fails, `deploy.sh` treats the provider as unavailable and falls back
to reading `.env` without saying so, the same as it does when Doppler is unavailable.
Run the verify command above before relying on the switch.

Each server needs its own CLI install and its own `/etc/infisical/env`.

Not covered: `mitch-mail` still tries `doppler secrets download` as a last-resort
fallback inside its own process (`rust/crates/mitch-mail/src/config.rs`). That only
matters when its environment lacks the mail credentials, which compose provides.

## Troubleshooting

**"[deploy] Waiting for the deployment lock..."** is printed on every run, before
`flock`. It does not mean the deploy is blocked on the lock.

**`deploy.sh` exits right after that line with no error.** The cause is the `.env`
fallback. When `secrets_available` is false (CLI not installed, wrong domain, bad
token, wrong environment), `deploy.sh` greps `.env` for `NTFY_TOPIC`, `NTFY_USER` and
`NTFY_PASS`. Under `set -euo pipefail`, a grep with no match makes the script exit
silently. Diagnose with:

```sh
set -a; . /etc/infisical/env; set +a
bash -x /usr/local/bin/deploy.sh 2>&1 | tail -30
```

The last command shown is where it stopped. Also check `command -v infisical` and the
verify command above.

**A stuck deployment lock** (`/tmp/mitch-pro-deploy.lock`) can be inspected with
`lslocks | grep -i deploy` (`fuser` may not be installed).

## Testing

`bash tests/test_secrets_provider.sh` checks the four copies are identical, then runs
the block against stand-in CLIs and checks which commands each provider makes. It does not talk to a real Infisical or
Doppler.
