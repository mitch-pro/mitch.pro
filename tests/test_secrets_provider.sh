#!/usr/bin/env bash
# Checks the secrets-provider block in the host scripts: that all copies are identical,
# and that it works against stand-in `doppler` and `infisical` commands that record
# how they were called. Run: bash tests/test_secrets_provider.sh
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/bin"

for cli in doppler infisical; do
    cat > "$WORK/bin/$cli" <<STUB
#!/usr/bin/env bash
echo "$cli \$*" >> "$WORK/calls"
case "\$*" in
    *"secrets get"*) echo "value-from-$cli" ;;
esac
exit 0
STUB
    chmod +x "$WORK/bin/$cli"
done
export PATH="$WORK/bin:$PATH"

fail() { echo "FAIL: $*" >&2; exit 1; }
expect_call() { grep -qx -- "$1" "$WORK/calls" || { cat "$WORK/calls" >&2; fail "expected call: $1"; }; }

# The block is inlined in each root-run script (not sourced from a shared file).
# Extract it from each, ignoring indentation, and require the copies to match.
SCRIPTS=(tools/deploy.sh newsletter.sh tools/update_ip.sh tools/add_cloudflare_domain.sh)
extract() { sed -n '/^ *# >>> secrets-provider/,/^ *# <<< secrets-provider/p' "$ROOT/$1" | sed 's/^ *//'; }
extract "${SCRIPTS[0]}" > "$WORK/block.sh"
[ -s "$WORK/block.sh" ] || fail "no secrets-provider block in ${SCRIPTS[0]}"
for script in "${SCRIPTS[@]}"; do
    extract "$script" | cmp -s - "$WORK/block.sh" || fail "secrets-provider block in $script differs from ${SCRIPTS[0]}"
done
# shellcheck disable=SC1091
. "$WORK/block.sh"

check_provider() {  # <flag line or empty> <expected provider>
    : > "$WORK/.env"; [ -n "$1" ] && echo "$1" > "$WORK/.env"
    unset INFISICAL || true
    secrets_init "$WORK/.env"
    [ "$SECRETS_PROVIDER" = "$2" ] || fail "'$1' gave $SECRETS_PROVIDER, wanted $2"
}

check_provider "" doppler
check_provider "INFISICAL=False" doppler
check_provider "INFISICAL=True" infisical
check_provider 'INFISICAL="true"' infisical
check_provider "INFISICAL=1" infisical
INFISICAL=True secrets_init "$WORK/missing.env"; [ "$SECRETS_PROVIDER" = infisical ] || fail "environment flag"

# Doppler (default): the calls deploy.sh made before the helper existed.
: > "$WORK/calls"; check_provider "" doppler
secrets_available || fail "doppler available"
[ "$(secrets_get NTFY_TOPIC)" = "value-from-doppler" ] || fail "doppler get"
secrets_set A b || fail "doppler set"
secrets_run docker compose up -d
expect_call "doppler secrets download --format json"
expect_call "doppler secrets get NTFY_TOPIC --plain"
expect_call "doppler secrets set A=b"
expect_call "doppler run -- docker compose up -d"
! grep -q infisical "$WORK/calls" || fail "doppler mode called infisical"

# Infisical: the same operations through its CLI, nothing from .env but the flag.
: > "$WORK/calls"; check_provider "INFISICAL=True" infisical
[ "$(secrets_cli)" = infisical ] || fail "cli name"
secrets_available || fail "infisical available"
[ "$(secrets_get NTFY_TOPIC)" = "value-from-infisical" ] || fail "infisical get"
secrets_set A b || fail "infisical set"
secrets_run docker compose up -d
expect_call "infisical export --format=json"
expect_call "infisical secrets get NTFY_TOPIC --plain"
expect_call "infisical secrets set A=b"
expect_call "infisical run -- docker compose up -d"
! grep -q doppler "$WORK/calls" || fail "infisical mode called doppler"

# A missing CLI is "not available", not an error.
PATH="/usr/bin:/bin" bash -c ". '$WORK/block.sh'; INFISICAL=True secrets_init /nonexistent; ! secrets_available" || fail "missing cli"

echo "secrets provider: ok"
