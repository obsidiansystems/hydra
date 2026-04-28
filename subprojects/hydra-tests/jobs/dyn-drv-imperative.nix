# Minimal jobset used by content-addressed/dyn-drv-imperative.t — NOT
# actually a dynamic derivation. The daemon's BuildResult currently does
# not surface CA realisations (see drv-daemon TODO), so the client side
# of the test relies on a plain input-addressed derivation whose output
# path is fully determined by its drv hash.
#
# The dynamic-derivations behaviour itself is unchanged in the queue
# runner; this test only exercises the new
# "BuildPaths -> ad-hoc Build -> queue runner -> daemon wakeup" path.
let
  cfg = import ./config.nix;
in
{
  hello = cfg.mkDerivation {
    name = "hello-imperative";
    builder = "/bin/sh";
    args = [
      "-c"
      ''
        mkdir -p $out
        echo "hello from drv-daemon" > $out/result
      ''
    ];
  };
}
