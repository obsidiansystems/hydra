#!/bin/sh

. ./foreman/common.sh

wait_for_queue_runner_grpc

export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

CONFIG="$HYDRA_DATA/builder.toml"

# Generate a config for the builder if it doesn't exist
if [ ! -f "$CONFIG" ]; then
    cat <<EOF > "$CONFIG"
supportedFeatures = ["nixos-test", "benchmark", "big-parallel", "kvm", "builder-rpc-v0", "recursive-nix"]
EOF
fi

exec hydra-builder --config-path "$CONFIG"
