{
  lib,
  version,

  rustPlatform,

  nixComponents,
  protobuf,
  pkg-config,
  rust-jemalloc-sys,
  withOtel ? false,
}:

rustPlatform.buildRustPackage {
  pname = "hydra-queue-runner";
  inherit version;

  src = lib.fileset.toSource {
    root = ../..;
    fileset = lib.fileset.unions [
      ../../Cargo.toml
      ../../Cargo.lock
      ../../.cargo
      ../../.sqlx
      ../../subprojects/hydra-queue-runner/Cargo.toml
      ../../subprojects/hydra-queue-runner/src
      ../../subprojects/hydra-queue-runner/examples
      ../../subprojects/crates
      # For unit tests which want to spin up a fresh database
      ../../subprojects/hydra/sql/hydra.sql
      ../../subprojects/proto
    ];
  };

  cargoLock = {
    lockFile = ../../Cargo.lock;
    outputHashes = import ../../cargo-output-hashes.nix;
  };

  # The source fileset above intentionally excludes the other Rust
  # binary crates, so drop them from the workspace members to keep
  # cargo from trying to load their (absent) manifests.
  postPatch = ''
    sed -i '/hydra-queue-runner/!{/"subprojects\/hydra-/d;}' Cargo.toml
  '';

  buildAndTestSubdir = "subprojects/hydra-queue-runner";
  buildFeatures = lib.optional withOtel "otel";

  nativeBuildInputs = [
    pkg-config
    protobuf
  ];

  buildInputs = [
    nixComponents.nix-main
    protobuf
    rust-jemalloc-sys
  ];

  # FIXME: get these passing in a prod build
  doCheck = false;

  meta.description = "Hydra queue runner (Rust)";
}
