#!/bin/sh

. ./foreman/common.sh

wait_for_queue_runner_grpc

export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"

exec hydra-builder --supported-features "nixos-test benchmark big-parallel kvm builder-rpc-v0 recursive-nix"
