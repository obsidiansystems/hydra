use warnings;
use strict;

package DrvDaemonContext;

# Spin up the full stack required for builds submitted directly through
# hydra-drv-daemon: an upstream nix-daemon (so the drv-daemon has
# something to proxy reads / .drv uploads to), the drv-daemon itself,
# and a queue-runner + builder pair that picks up the rows it inserts.
#
# Returns an object whose DESTROY tears everything down. Use
# `daemon_socket` to point a `nix-build` / `nix-store` invocation at
# the drv-daemon, and `nix_remote_url` for the same over `NIX_REMOTE`.

use IPC::Run;
use LWP::UserAgent;
use URI::Escape qw(uri_escape);
use ProcessGroup;
use QueueRunnerContext qw(
    start_queue_runner
    start_builder
    wait_for_socket
    wait_for_url
    wait_for_builds
);

our @ISA = qw(Exporter);
our @EXPORT = qw();

sub new {
    my ($class, $ctx) = @_;
    ref $ctx eq 'HydraTestContext'
        or die "DrvDaemonContext requires a HydraTestContext\n";

    my $tmpdir = $ctx->{tmpdir};
    my $upstream_sock = "$tmpdir/upstream-nix-daemon.sock";
    my $daemon_sock = "$tmpdir/drv-daemon.sock";

    my $pg = ProcessGroup->new(env => { %{$ctx->{central_env}} });

    # Upstream nix-daemon via socat
    $pg->spawn(
        "upstream-nix-daemon",
        [
            "socat",
            "UNIX-LISTEN:$upstream_sock,fork,reuseaddr,unlink-early",
            "EXEC:nix-daemon --stdio,nofork",
        ],
    );
    wait_for_socket($upstream_sock)
        or die "upstream nix-daemon socket did not appear at $upstream_sock\n";

    # drv-daemon
    $pg->spawn(
        "drv-daemon",
        [
            "hydra-drv-daemon",
            "--socket",          $daemon_sock,
            "--upstream-socket", $upstream_sock,
            "--db-url",          $ctx->{central}{hydra_database_url},
            "--store-dir",       $ctx->{central}{nix_store_dir},
        ],
        env => { RUST_LOG => "hydra_drv_daemon=debug,info" },
    );
    wait_for_socket($daemon_sock)
        or die "hydra-drv-daemon socket did not appear at $daemon_sock\n";

    # Queue runner
    my ($base_url, $grpc_addr) =
        start_queue_runner($pg, $ctx,
            queue_monitor_loop => 1,
            rust_log           => "queue_runner=debug,info",
        );

    # Builder
    start_builder($pg, $ctx, $grpc_addr,
        rust_log => "hydra_builder=debug,info",
    );

    my $ua = LWP::UserAgent->new(timeout => 2);
    wait_for_url($ua, "$base_url/status/machines", sub {
        shift->decoded_content =~ /"hostname"/;
    }) or die "Timed out waiting for builder to register\n";

    my $self = bless {
        ctx         => $ctx,
        pg          => $pg,
        daemon_sock => $daemon_sock,
        base_url    => $base_url,
        ua          => $ua,
    }, $class;

    return $self;
}

sub daemon_socket { return $_[0]->{daemon_sock}; }

sub nix_remote_url {
    my ($self) = @_;
    my $store_dir = $self->{ctx}{central}{nix_store_dir};
    return
        "unix://" . uri_escape($self->{daemon_sock}, "^A-Za-z0-9\\-_.~/:")
      . "?store=" . uri_escape($store_dir,           "^A-Za-z0-9\\-_.~/:");
}

sub pump_logs {
    my ($self) = @_;
    $self->{pg}->pump_logs;
}

sub wait_for_builds_to_finish {
    my ($self, @build_ids) = @_;
    wait_for_builds($self->{ua}, $self->{base_url}, $self->{pg}, @build_ids);
}

sub run_cmd {
    my ($self, $timeout, @cmd) = @_;
    my %env = (
        %{ $self->{ctx}{central_env} },
        NIX_REMOTE => $self->nix_remote_url,
    );

    my ($cmd_in, $cmd_out, $cmd_err) = ("", "", "");
    my $h;
    {
        local @ENV{keys %env} = values %env;
        local $ENV{NO_COLOR} = "1";
        $h = IPC::Run::start(\@cmd, \$cmd_in, \$cmd_out, \$cmd_err);
    }

    my $deadline = time() + $timeout;
    while (time() < $deadline) {
        eval { $h->pump_nb };
        my $err = $@;
        $self->pump_logs;
        if ($err) {
            return (1, $cmd_out, $cmd_err . "\n[run_cmd: pump_nb error: $err]");
        }
        if (!$h->pumpable) {
            $h->finish;
            my $rc = scalar $h->result;
            unless (defined $rc) {
                return (
                    1, $cmd_out,
                    $cmd_err . "\n[run_cmd: IPC::Run::result returned undef; child likely crashed without a clean exit]",
                );
            }
            return ($rc, $cmd_out, $cmd_err);
        }
        select(undef, undef, undef, 0.5);
    }
    eval { $h->kill_kill };
    return (1, $cmd_out, $cmd_err . "\n[run_cmd: timed out after ${timeout}s]");
}

sub stop {
    my ($self) = @_;
    $self->{pg}->stop;
}

sub DESTROY {
    my ($self) = @_;
    $self->stop;
}

1;
