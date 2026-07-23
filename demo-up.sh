#!/usr/bin/env bash
# Bring up the dyn-drv demo: isolated nix daemon + foreman + a dyn-drv jobset.
#
# The system nix daemon rejects ca-derivations and dynamic-derivations
# (/etc/nix/nix.conf has no experimental-features line, and that setting is
# not client-overridable across the daemon boundary), so the demo runs its
# own daemon over an isolated store, the same way the perl test harness does.
set -euo pipefail

cd "$(dirname "$0")"
HYDRA_DATA=$(pwd)/.hydra-data
ROOT=$HYDRA_DATA/nixroot
STORE=$ROOT/nix/store
STATE=$ROOT/nix/var/nix
SOCK=$STATE/daemon-socket/socket

mkdir -p "$STORE" "$STATE" "$ROOT/etc" "$HYDRA_DATA"

# sandbox=false because the test jobs reach coreutils through PATH without
# declaring it as an input; auto-allocate-uids needs the sandbox, so a
# sandboxed daemon would need build users this machine does not have.
cat > "$ROOT/etc/nix.conf" <<EOF
sandbox = false
experimental-features = nix-command flakes ca-derivations dynamic-derivations recursive-nix fetch-tree pipe-operators
EOF

cat > "$HYDRA_DATA/demo-env.sh" <<EOF
export NIX_STORE_DIR="$STORE"
export NIX_STATE_DIR="$STATE"
export NIX_CONF_DIR="$ROOT/etc"
export NIX_DAEMON_SOCKET_PATH="$SOCK"
export NIX_REMOTE="unix://$SOCK?root=$ROOT&store=$STORE"
EOF

if [ ! -S "$SOCK" ]; then
    env NIX_REMOTE="local?root=$ROOT&store=$STORE" \
        NIX_STORE_DIR="$STORE" NIX_STATE_DIR="$STATE" NIX_CONF_DIR="$ROOT/etc" \
        NIX_DAEMON_SOCKET_PATH="$SOCK" NIX_CONFIG='trusted-users = *' \
        nix-daemon > "$ROOT/daemon.log" 2>&1 &
    for _ in $(seq 1 80); do [ -S "$SOCK" ] && break; sleep 0.5; done
    [ -S "$SOCK" ] || { echo "nix-daemon did not start; see $ROOT/daemon.log" >&2; exit 1; }
fi

. "$HYDRA_DATA/demo-env.sh"
nix-store --init

# The jobs reference coreutils/bash/nix by absolute store path, so config.nix
# must be rendered from inside the dev shell, not the host environment.
JOBS=$HYDRA_DATA/jobs
rm -rf "$JOBS"
cp -r subprojects/hydra-tests/jobs "$JOBS"
python3 - "$JOBS/config.nix.in" "$JOBS/config.nix" \
    "$(dirname "$(command -v install)")" "$(dirname "$(command -v nix)")" \
    "$(command -v bash)" "$(nix --extra-experimental-features nix-command config show system)" <<'PY'
import sys
src, dst, coreutils, nixbin, bash, system = sys.argv[1:7]
t = open(src).read()
for k, v in (("@testPath@", coreutils), ("@nixBinDir@", nixbin),
             ("@bash@", bash), ("@system@", system)):
    t = t.replace(k, v)
open(dst, "w").write(t)
PY

echo "nix daemon:  $SOCK"
echo "jobs dir:    $JOBS"
echo "env file:    $HYDRA_DATA/demo-env.sh"
echo "now run:     foreman start"
echo "then:        ./demo-jobset.sh"
