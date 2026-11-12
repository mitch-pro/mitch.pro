#!/usr/bin/env bash
# newsletter.sh - Wrapper script that runs newsletter.py with the secrets from
# Doppler, or from Infisical when INFISICAL=True is set in .env
# (tools/secrets-provider.sh).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# shellcheck source=tools/secrets-provider.sh
. "$SCRIPT_DIR/tools/secrets-provider.sh"
secrets_init "$SCRIPT_DIR/.env"

if secrets_available; then
    secrets_run ./newsletter.py "$@"
    exit $?
elif command -v sudo &>/dev/null; then
    exec sudo "$(secrets_cli)" run -- ./newsletter.py "$@"
else
    exec ./newsletter.py "$@"
fi
