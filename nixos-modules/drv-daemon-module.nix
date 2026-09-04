{
  config,
  pkgs,
  lib,
  ...
}:
let
  cfg = config.services.hydra-drv-daemon-dev;

  user = "hydra-drv-daemon";

  format = pkgs.formats.toml { };

  otel = import ./otel.nix { inherit lib; };
in
{
  options = {
    services.hydra-drv-daemon-dev = {
      enable = lib.mkEnableOption "Hydra drv-daemon (turns IFD / imperative builds into ad-hoc Hydra Builds)";

      settings = lib.mkOption {
        description = "Settings for the drv-daemon";
        type = lib.types.submodule {
          options = {
            dbUrl = lib.mkOption {
              description = "Postgresql database url";
              type = lib.types.singleLineStr;
              default = "postgres://hydra@%2Frun%2Fpostgresql:5432/hydra";
            };
            maxDbConnections = lib.mkOption {
              description = "Postgresql maximum db connections";
              type = lib.types.ints.positive;
              default = 4;
            };
            upstreamSocket = lib.mkOption {
              description = ''
                Upstream nix-daemon socket that the drv-daemon proxies read
                ops and `.drv` uploads to.
              '';
              type = lib.types.path;
              default = "/nix/var/nix/daemon-socket/socket";
            };
            storeDir = lib.mkOption {
              description = "Nix store directory.";
              type = lib.types.path;
              default = "/nix/store";
            };
          };
        };
        default = { };
      };

      socketPath = lib.mkOption {
        type = lib.types.path;
        default = "/run/hydra-drv-daemon/socket";
        description = ''
          Socket path used by clients to reach the daemon.
        '';
      };

      otel = otel.mkOtelOption {
        component = "the drv-daemon";
        binary = "hydra-drv-daemon";
      };

      package = lib.mkOption {
        type = lib.types.package;
        # `withOtel` is a knob on the rust workspace, not on this crate: cargo
        # resolves features once for the whole workspace build.
        default =
          (pkgs.hydraComponents.overrideScope (_: _: { withOtel = cfg.otel.enable; })).hydra-drv-daemon;
        defaultText = lib.literalExpression "pkgs.hydraComponents.hydra-drv-daemon";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.services.hydra-drv-daemon-dev = {
      description = "Hydra drv-daemon (ad-hoc Build dispatcher)";

      requires = [
        "nix-daemon.socket"
        "hydra-drv-daemon-dev.socket"
      ];
      after = [
        # sets up database
        "hydra-init.service"
        "network.target"
      ];
      wantedBy = [ "multi-user.target" ];
      # The daemon has no hot-reload; restart it when the config changes.
      restartTriggers = [ config.environment.etc."hydra/drv-daemon.toml".source ];

      environment = {
        RUST_BACKTRACE = "1";
      }
      // otel.otelEnv cfg.otel;

      serviceConfig = {
        Type = "notify";
        Restart = "always";
        RestartSec = "5s";

        ExecStart = lib.escapeShellArgs [
          "${cfg.package}/bin/hydra-drv-daemon"
          "--socket"
          "-"
          "--config-path"
          "/etc/hydra/drv-daemon.toml"
        ];

        User = user;
        Group = "hydra";

        PrivateNetwork = false;
        SystemCallFilter = [
          "@system-service"
          "~@privileged"
          "~@resources"
        ];
        ReadWritePaths = [
          cfg.settings.upstreamSocket
        ]
        ++ lib.optionals (lib.hasInfix "%2Frun%2Fpostgresql" cfg.settings.dbUrl) [
          "/run/postgresql/.s.PGSQL.${toString config.services.postgresql.settings.port}"
        ];
        ReadOnlyPaths = [ "/nix/" ];

        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        PrivateDevices = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        PrivateMounts = true;
        RemoveIPC = true;
        UMask = "0022";

        CapabilityBoundingSet = "";
        NoNewPrivileges = true;

        ProtectKernelModules = true;
        SystemCallArchitectures = "native";
        ProtectKernelLogs = true;
        ProtectClock = true;

        RestrictAddressFamilies = "";

        LockPersonality = true;
        ProtectHostname = true;
        RestrictRealtime = true;
        MemoryDenyWriteExecute = true;
        PrivateUsers = true;
        RestrictNamespaces = true;
      };
    };

    # systemd owns the socket file: it creates the parent directory,
    # and the mode below is what limits build submission to the hydra
    # group rather than every local user.
    systemd.sockets.hydra-drv-daemon-dev = {
      description = "Hydra drv-daemon socket";
      wantedBy = [ "sockets.target" ];
      socketConfig = {
        ListenStream = cfg.socketPath;
        SocketUser = user;
        SocketGroup = "hydra";
        SocketMode = "0660";
        FileDescriptorName = "daemon";
        Service = "hydra-drv-daemon-dev.service";
      };
    };

    environment.etc."hydra/drv-daemon.toml".source = format.generate "drv-daemon.toml" (
      lib.filterAttrsRecursive (_: v: v != null) cfg.settings
    );

    services.postgresql.identMap = ''
      hydra-users ${user} hydra
    '';

    # Same trust as the queue-runner: the daemon talks to the upstream
    # nix-daemon on behalf of its clients, and some of what it forwards
    # may need a trusted user.
    nix.settings = {
      trusted-users = [ user ];
    };

    users = {
      groups.hydra = { };
      users.${user} = {
        group = "hydra";
        isSystemUser = true;
      };
    };
  };
}
