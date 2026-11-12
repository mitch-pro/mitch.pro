# Secrets providers

Secrets (API keys, `DATA_ENCRYPTION_KEY`, ntfy credentials) reach the containers
as environment variables, injected by the host when it starts them. The app never
talks to a secrets manager. Two providers are supported, chosen on the host:

| Provider | Selected by | Injects with |
| --- | --- | --- |
| Doppler | default | `doppler run -- docker compose ...` |
| Infisical | `INFISICAL=True` in the repo's `.env` | `infisical run -- docker compose ...` |

`tools/secrets-provider.sh` implements the choice. `tools/deploy.sh`,
`newsletter.sh`, `tools/update_ip.sh` and `tools/add_cloudflare_domain.sh` source it,
so one flag moves them all. With the flag unset, behaviour is the same as before
Infisical support was added.

## Configuring Infisical

Only the flag goes in `.env`. Everything else is configured on the host by root,
the way the Doppler CLI is:

1. Install the Infisical CLI for root.
2. Authenticate root: `infisical login`, or a machine identity (for example
   `INFISICAL_TOKEN`) in root's own environment, not in the repo.
3. In the project directory run `infisical init`, which writes `.infisical.json`
   (project id and default environment). The CLI uses `dev` unless that file or
   `--env` says otherwise, so set the production environment there.
4. Check it as root: `infisical export --format=json` should list the secrets.

Switching: copy the secrets across in Infisical, add `INFISICAL=True` to `.env`, and
deploy. If `infisical export` fails, `deploy.sh` treats the provider as unavailable and
falls back to reading `.env` without saying so, the same as it does when Doppler is
unavailable. Run the check in step 4 as root before relying on the switch.

Not covered: `mitch-mail` still tries `doppler secrets download` as a last-resort
fallback inside its own process (`rust/crates/mitch-mail/src/config.rs`). That only
matters when its environment lacks the mail credentials, which compose provides.

## Testing

`bash tests/test_secrets_provider.sh` runs the helper against stand-in CLIs and
checks which commands each provider makes. It does not talk to a real Infisical or
Doppler.
