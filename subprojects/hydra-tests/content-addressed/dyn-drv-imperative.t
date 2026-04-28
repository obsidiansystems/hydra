use feature 'unicode_strings';
use strict;
use warnings;
use Setup;
use Test2::V0;
use DrvDaemonContext;
use Hydra::Helper::Exec;

# Submits a dynamic-derivation wrapper to the live drv-daemon via
# `nix-store --realise`, demonstrating that imperative builds (no Hydra
# jobset / evaluator round-trip) end up scheduled in the queue runner
# under the auto-created `adhoc/adhoc` jobset and resolve correctly.

my $ctx = test_context(
    nix_config => qq|
    experimental-features = ca-derivations dynamic-derivations
    |,
);

my $jobsdir = $ctx->jobsdir;

my $drv;
{
    local @ENV{keys %{$ctx->{central_env}}} = values %{$ctx->{central_env}};
    my ($res, $stdout, $stderr) = captureStdoutStderr(60,
        "nix-instantiate", "$jobsdir/dyn-drv-imperative.nix", "-A", "hello",
    );
    if ($res) {
        chomp $stderr;
        diag("nix-instantiate failed: $stderr");
        die "nix-instantiate failed\n";
    }
    chomp $stdout;
    $stdout =~ s/!.*$//;
    $drv = $stdout;
}

ok($drv =~ m{\.drv$}, "instantiated to $drv");

my $stack = DrvDaemonContext->new($ctx);

my ($res, $stdout, $stderr) = $stack->run_cmd(900,
    "nix-store", "--realise",
    "--add-root", "$ctx->{tmpdir}/result", "--indirect",
    $drv,
);
if ($res) {
    chomp $stderr;
    diag("nix-store --realise failed: $stderr");
}
$stack->pump_logs;
is(($res // 0) + 0, 0, "nix-store --realise via drv-daemon succeeds");

my $db = $ctx->db();
my @builds = $db->resultset('Builds')->search(
    { 'jobset.project' => 'adhoc', 'jobset.name' => 'adhoc' },
    { join => 'jobset', order_by => 'me.id desc' },
);
ok(scalar(@builds) >= 1, "drv-daemon created an ad-hoc Builds row");

my $build = $builds[0];
$build->discard_changes;
is($build->finished, 1, "ad-hoc build is marked finished");
is($build->buildstatus, 0, "ad-hoc build succeeded");
is($build->drvpath, $drv, "ad-hoc build drvpath matches the submitted derivation");

my $result_link = "$ctx->{tmpdir}/result";
ok(-l $result_link || -e $result_link, "nix-store produced an output symlink");

$stack->stop;

done_testing;
