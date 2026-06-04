{
  system,
  nixpkgs,
  common,
}:

(import (nixpkgs + "/nixos/lib/testing-python.nix") { inherit system; }).makeTest {
  name = "hydra";
  globalTimeout = 30 * 24 * 60 * 60;
  sshBackdoor.enable = true;

  nodes = {
    server =
      { ... }:
      {
        imports = [ common.serverConfig ];
        virtualisation.forwardPorts = [
          {
            from = "host";
            host.port = 3000;
            guest.port = 3000;
          }
        ];

        networking.firewall.allowedTCPPorts = [ 3000 ];
      };
  }
  // (builtins.listToAttrs (
    builtins.genList (i: {
      name = "builder${toString i}";
      value = common.builderConfig;
    }) 16
  ));
  skipLint = true;
  testScript = ''
    import time

    server.start()
    server.wait_for_unit("multi-user.target")
    server.wait_for_unit("hydra-queue-runner-dev.service")
    server.wait_for_open_port(3000)
    server.succeed("su -l hydra -c \"hydra-create-user root --email-address 'alice@example.org' --password password --role admin\"")

    start_all()

    while True:
      time.sleep(60)
  '';
}
