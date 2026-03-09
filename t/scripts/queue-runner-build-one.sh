#!/usr/bin/env bash

set -euo pipefail
set -x

get_random_port() {
    local port_min=$1
    local port_max=$2
    local port

    while :; do
        port=$(shuf -i "${port_min}-${port_max}" -n 1)

        if ! ss -ltn "( sport = :${port} )" 2>/dev/null | grep -q ":${port}"; then
            echo "${port}"
            return 0
        fi
    done
}

cleanup() {
    kill "${QUEUE_RUNNER_PID:-}" "${BUILDER_PID:-}" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "${CONFIG_DIR:-}" 2>/dev/null || true
}

if [[ $# -eq 0 ]]; then
    echo "Usage: $0 <build_id>"
    exit 1
fi

BUILD_ID="$1"

trap cleanup EXIT INT TERM

GRPC_PORT=$(get_random_port 5000 9999)
HTTP_PORT=$(get_random_port 10000 19999)

# Build a postgres:// URL from the Perl DBI string that the test harness sets
# (e.g. HYDRA_DBI='dbi:Pg:dbname=test;host=127.0.0.1;port=15437').
build_db_url() {
    local dbi="${HYDRA_DBI:-}"
    local dbname port host user

    dbname=$(echo "$dbi" | sed -n 's/.*dbname=\([^;]*\).*/\1/p')
    port=$(echo "$dbi" | sed -n 's/.*port=\([^;]*\).*/\1/p')
    host=$(echo "$dbi" | sed -n 's/.*host=\([^;]*\).*/\1/p')
    user=$(echo "$dbi" | sed -n 's/.*user=\([^;]*\).*/\1/p')

    : "${dbname:=hydra}"
    : "${port:=5432}"
    : "${host:=${PGHOST:-/tmp}}"
    : "${user:=$(id -un)}"

    # Percent-encode Unix socket dir paths.
    if [[ "$host" == /* ]]; then
        host=$(printf '%s' "$host" | sed 's|/|%2F|g')
    fi

    echo "postgres://${user}@${host}:${port}/${dbname}"
}

CONFIG_DIR=$(mktemp -d)
CONFIG_FILE="${CONFIG_DIR}/config.toml"
DB_URL=$(build_db_url)

# Read store settings from the Apache-style HYDRA_CONFIG if present.
DEST_STORE_URI="" USE_SUBSTITUTES=""
if [[ -n "${HYDRA_CONFIG:-}" && -f "${HYDRA_CONFIG}" ]]; then
    DEST_STORE_URI=$(sed -n 's/^\s*store_uri\s*=\s*//p' "${HYDRA_CONFIG}" | head -1 | xargs || true)
    USE_SUBSTITUTES=$(sed -n 's/^\s*use-substitutes\s*=\s*//p' "${HYDRA_CONFIG}" | head -1 | xargs || true)
fi

{
    echo "dbUrl = \"${DB_URL}\""
    echo "hydraDataDir = \"${CONFIG_DIR}/data\""
    [[ -n "${DEST_STORE_URI}" ]] && echo "destStoreUri = \"${DEST_STORE_URI}\""
    [[ "${USE_SUBSTITUTES}" == "1" ]] && echo "useSubstitutes = true"
} > "${CONFIG_FILE}"

RUST_LOG=queue_runner=debug,info NO_COLOR=1 hydra-queue-runner \
    --config-path "${CONFIG_FILE}" \
    --rest-bind "[::]:${HTTP_PORT}" \
    --grpc-bind "[::]:${GRPC_PORT}" \
    --disable-queue-monitor-loop &
QUEUE_RUNNER_PID=$!

# Wait for the REST server to become available before starting the builder.
for _ in $(seq 1 30); do
    curl -sf "http://[::1]:${HTTP_PORT}/status" >/dev/null 2>&1 && break
    sleep 0.5
done

RUST_LOG=builder=debug,info NO_COLOR=1 hydra-builder --gateway-endpoint "http://[::1]:${GRPC_PORT}" &
BUILDER_PID=$!

# Wait for the builder to register as a machine.
for _ in $(seq 1 30); do
    curl -sf "http://[::1]:${HTTP_PORT}/status/machines" 2>/dev/null | grep -q '"hostname"' && break
    sleep 0.5
done

# Submit build and poll until it finishes.
curl -s --fail -X POST \
    --json "{\"buildId\": ${BUILD_ID}}" \
    "http://[::1]:${HTTP_PORT}/build_one"
sleep 2

while true; do
    status=$(curl -s "http://[::1]:${HTTP_PORT}/status/build/${BUILD_ID}/active")
    [[ "${status}" == *"true"* ]] || break
    sleep 2
done
