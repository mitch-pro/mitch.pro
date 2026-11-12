#!/usr/bin/env bash
# tools/secrets-provider.sh - sourced by deploy.sh and the other host scripts that
# need secrets. Not run directly.
#
# Doppler is the default provider. `INFISICAL=True` in the repo's .env (or in the
# environment) switches every script to Infisical. Nothing else about Infisical
# goes in .env: the CLI is configured by root, outside the repo (login or a machine
# identity in root's environment, plus `infisical init` in the project directory so
# `.infisical.json` names the project and default environment), the same way the
# Doppler CLI is today.
#
# Functions (after `secrets_init <path to .env>`):
#   secrets_cli                the CLI's name (doppler or infisical)
#   secrets_available          the CLI works and can read this project's secrets
#   secrets_get NAME           the plain value, or nothing
#   secrets_set NAME VALUE     0 on success
#   secrets_run CMD...         run CMD with the secrets in its environment
# Set SECRETS_SUDO=1 to retry get/set with `sudo -n` when the plain call fails.

secrets_init() {
    local env_file="${1:-}" flag="${INFISICAL:-}"
    if [ -z "$flag" ] && [ -f "$env_file" ]; then
        flag=$(grep -E '^INFISICAL=' "$env_file" | tail -n1 | cut -d= -f2- | tr -d '"' | tr -d "'" || true)
    fi
    case "$(printf '%s' "$flag" | tr '[:upper:]' '[:lower:]')" in
        true|1|yes|on) SECRETS_PROVIDER=infisical ;;
        *) SECRETS_PROVIDER=doppler ;;
    esac
    # Doppler's resolver setting, unchanged from before this file existed.
    export DOPPLER_ENABLE_DNS_RESOLVER=true
}

# The CLI's name, for callers that need it (for example under sudo).
secrets_cli() {
    if [ "${SECRETS_PROVIDER:-doppler}" = infisical ]; then echo infisical; else echo doppler; fi
}

secrets_available() {
    if [ "${SECRETS_PROVIDER:-doppler}" = infisical ]; then
        command -v infisical &> /dev/null && infisical export --format=json &> /dev/null
    else
        command -v doppler &> /dev/null && doppler secrets download --format json &> /dev/null
    fi
}

_secrets_get_once() {
    if [ "${SECRETS_PROVIDER:-doppler}" = infisical ]; then
        "$@" infisical secrets get "$NAME_" --plain 2>/dev/null
    else
        "$@" doppler secrets get "$NAME_" --plain 2>/dev/null
    fi
}

secrets_get() {
    local NAME_="$1" val=""
    val=$(_secrets_get_once || echo "")
    if [ -z "$val" ] && [ "${SECRETS_SUDO:-0}" = 1 ] && command -v sudo &> /dev/null; then
        val=$(_secrets_get_once sudo -n || echo "")
    fi
    printf '%s' "$val"
}

secrets_set() {
    local k="$1" v="$2"
    if [ "${SECRETS_PROVIDER:-doppler}" = infisical ]; then
        command -v infisical &> /dev/null && infisical secrets set "$k=$v" &> /dev/null && return 0
        [ "${SECRETS_SUDO:-0}" = 1 ] && command -v sudo &> /dev/null && sudo -n infisical secrets set "$k=$v" &> /dev/null && return 0
    else
        command -v doppler &> /dev/null && doppler secrets set "$k=$v" &> /dev/null && return 0
        [ "${SECRETS_SUDO:-0}" = 1 ] && command -v sudo &> /dev/null && sudo -n doppler secrets set "$k=$v" &> /dev/null && return 0
    fi
    return 1
}

secrets_run() {
    if [ "${SECRETS_PROVIDER:-doppler}" = infisical ]; then
        infisical run -- "$@"
    else
        doppler run -- "$@"
    fi
}
