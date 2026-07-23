#!/usr/bin/env bash
# Create the demo project + dyn-drv jobset against a running hydra-server.
set -euo pipefail
cd "$(dirname "$0")"
BASE=http://localhost:63333
JOBS=$(pwd)/.hydra-data/jobs
CJ=$(mktemp)
NIXEXPR=${1:-dyn-drv.nix}

curl -sf -c "$CJ" -X POST "$BASE/login" -H "Accept: application/json" \
    -H "Referer: $BASE/" --data-urlencode username=alice --data-urlencode password=foobar > /dev/null

curl -sf -b "$CJ" -X PUT "$BASE/project/demo" -H "Accept: application/json" \
    -H "Content-Type: application/json" -H "Referer: $BASE/" \
    -d '{"displayname":"demo","enabled":true,"visible":true,"owner":"alice"}' > /dev/null

curl -sf -b "$CJ" -X PUT "$BASE/jobset/demo/dyndrv" -H "Accept: application/json" \
    -H "Content-Type: application/json" -H "Referer: $BASE/" \
    -d "{\"description\":\"dyn-drv demo\",\"enabled\":1,\"visible\":true,\"keepnr\":1,\"checkinterval\":10,\"schedulingshares\":100,\"nixexprinput\":\"jobs\",\"nixexprpath\":\"$NIXEXPR\",\"inputs\":{\"jobs\":{\"type\":\"path\",\"value\":\"$JOBS\"}}}" > /dev/null

rm -f "$CJ"
echo "jobset ready: $BASE/jobset/demo/dyndrv"
