use warnings;
use strict;

package DrvDaemonContext;
use IO::Socket::IP;
use IPC::Run;
use JSON::PP;
use LWP::UserAgent;
use HTTP::Request;
use Hydra::Config;
use Hydra::Helper::Exec;

# Spin up the full stack required for builds submitted directly through
# the hydra-drv-daemon: an upstream nix-daemon (so the drv-daemon has
# something to proxy reads / .drv uploads to), the drv-daemon itself,
# and a queue-runner + builder pair that picks up the rows it inserts.
#
# Returns an object whose DESTROY tears everything down. Use
# `daemon_socket` to point a `nix-build` / `nix build` invocation at the
# drv-daemon, and `wait_for_idle` to block until the queue runner has no
# in-flight builds left.

our @ISA = qw(Exporter);
our @EXPORT = qw();

sub _get_random_port {
    my ($min, $max) = @_;
    while (1) {
        my $port = $min + int(rand($max - $min + 1));
        my $sock = IO::Socket::IP->new(
            LocalAddr => '::',
            LocalPort => $port,
            Proto     => 'tcp',
            ReuseAddr => 0,
        );
        if ($sock) {
            close($sock);
            return $port;
        }
    }
}

sub _wait_for {
    my ($ua, $url, $check) = @_;
    for my $i (1..60) {
        my $resp = $ua->get($url);
        if ($resp->is_success) {
            return 1 if !$check || $check->($resp);
        }
        select(undef, undef, undef, 0.5);
    }
    return 0;
}

sub _wait_for_socket {
    my ($path) = @_;
    for my $i (1..60) {
        return 1 if -S $path;
        select(undef, undef, undef, 0.5);
    }
    return 0;
}

sub _flush_stream {
    my ($label, $stream, $buf_ref, $final) = @_;
    return if $$buf_ref eq "";
    utf8::decode($$buf_ref) or warn "Invalid unicode in $label $stream.";
    while ($$buf_ref =~ s/^([^\n]*)\n//) {
        print STDERR "[$label $stream] $1\n";
    }
    if ($final && $$buf_ref ne "") {
        print STDERR "[$label $stream] $$buf_ref\n";
        $$buf_ref = "";
    }
}

sub _flush_harness {
    my ($self, $key, $final) = @_;
    my $entry = $self->{procs}{$key};
    _flush_stream($entry->{label}, "stdout", \$entry->{out}, $final);
    _flush_stream($entry->{label}, "stderr", \$entry->{err}, $final);
}

sub new {
    my ($class, $ctx) = @_;
    ref $ctx eq 'HydraTestContext'
        or die "DrvDaemonContext requires a HydraTestContext\n";

    my $tmpdir = $ctx->{tmpdir};
    my $upstream_sock = "$tmpdir/upstream-nix-daemon.sock";
    my $daemon_sock = "$tmpdir/drv-daemon.sock";

    my $self = bless {
        ctx           => $ctx,
        upstream_sock => $upstream_sock,
        daemon_sock   => $daemon_sock,
        procs         => {},
    }, $class;

    $self->_spawn_upstream;
    $self->_spawn_drv_daemon;
    $self->_spawn_queue_runner;
    $self->_spawn_builder;

    return $self;
}

sub _spawn {
    my ($self, $key, $label, $cmd, %opts) = @_;
    my %env = %{ $self->{ctx}{central_env} };
    if ($opts{env}) {
        for my $k (keys %{$opts{env}}) {
            $env{$k} = $opts{env}{$k};
        }
    }
    my $entry = {
        label => $label,
        in    => "",
        out   => "",
        err   => "",
    };
    {
        local @ENV{keys %env} = values %env;
        local $ENV{NO_COLOR} = "1";
        $entry->{harness} = IPC::Run::start(
            $cmd,
            \$entry->{in},
            \$entry->{out},
            \$entry->{err},
        );
    }
    $self->{procs}{$key} = $entry;
}

sub _spawn_upstream {
    my ($self) = @_;
    my $ctx = $self->{ctx};

    # socat keeps the listener alive across connections by forking
    # a fresh `nix-daemon --stdio` per accept. The legacy command
    # honours NIX_STORE_DIR / NIX_STATE_DIR from central_env, which
    # already points at the test's on-disk store.
    $self->_spawn(
        upstream => "Upstream nix daemon",
        [
            "socat",
            "UNIX-LISTEN:$self->{upstream_sock},fork,reuseaddr,unlink-early",
            "EXEC:nix-daemon --stdio,nofork",
        ],
    );
    _wait_for_socket($self->{upstream_sock})
        or die "upstream nix-daemon socket did not appear at $self->{upstream_sock}\n";
}

sub _spawn_drv_daemon {
    my ($self) = @_;
    my $ctx = $self->{ctx};
    my $db_url = $ctx->{central}{hydra_database_url};
    my $store_dir = $ctx->{central}{nix_store_dir};

    $self->_spawn(
        drv_daemon => "drv-daemon",
        [
            "hydra-drv-daemon",
            "--socket",          $self->{daemon_sock},
            "--upstream-socket", $self->{upstream_sock},
            "--db-url",          $db_url,
            "--store-dir",       $store_dir,
        ],
        env => { RUST_LOG => "hydra_drv_daemon=debug,info" },
    );
    _wait_for_socket($self->{daemon_sock})
        or die "hydra-drv-daemon socket did not appear at $self->{daemon_sock}\n";
}

sub _spawn_queue_runner {
    my ($self) = @_;
    my $ctx = $self->{ctx};

    my $grpc_port = _get_random_port(5000, 9999);
    my $http_port = _get_random_port(10000, 19999);

    my $config_dir = $ENV{T2_HARNESS_TEMP_DIR}
        // $ctx->{central}{hydra_data};
    my $config_file = "$config_dir/config.toml";

    my $hydra_config_file = $ctx->{central}{hydra_config_file};
    my $hydra_config = ($hydra_config_file && -f $hydra_config_file)
        ? Hydra::Config::loadConfig($hydra_config_file) : {};
    my $dest_store_uri = $hydra_config->{store_uri} // "";
    my $use_substitutes = $hydra_config->{'use-substitutes'} // "";

    my $db_url = $ctx->{central}{hydra_database_url};
    open(my $fh, '>', $config_file) or die "Cannot write $config_file: $!\n";
    print $fh "dbUrl = \"$db_url\"\n";
    print $fh "hydraDataDir = \"$config_dir/data\"\n";
    print $fh "remoteStoreAddr = [\"$dest_store_uri\"]\n" if $dest_store_uri ne "";
    print $fh "useSubstitutes = true\n" if $use_substitutes eq "1";
    close($fh);

    $self->{grpc_port} = $grpc_port;
    $self->{http_port} = $http_port;
    $self->{base_url}  = "http://[::1]:$http_port";

    $self->_spawn(
        queue_runner => "Queue runner",
        [
            "hydra-queue-runner",
            "--config-path", $config_file,
            "--rest-bind",   "[::]:$http_port",
            "--grpc-bind",   "[::]:$grpc_port",
        ],
        env => { RUST_LOG => "queue_runner=debug,info" },
    );

    my $ua = LWP::UserAgent->new(timeout => 2);
    $self->{ua} = $ua;
    _wait_for($ua, "$self->{base_url}/status")
        or die "Timed out waiting for queue-runner REST server\n";
}

sub _spawn_builder {
    my ($self) = @_;
    my $ctx = $self->{ctx};

    $self->_spawn(
        builder => "Builder",
        [
            "hydra-builder",
            "--gateway-endpoint", "http://[::1]:$self->{grpc_port}",
        ],
        env => {
            NIX_REMOTE    => $ctx->{builder}{nix_store_uri},
            NIX_CONF_DIR  => $ctx->{builder}{nix_conf_dir},
            NIX_STATE_DIR => $ctx->{builder}{nix_state_dir},
            NIX_STORE_DIR => $ctx->{builder}{nix_store_dir},
            RUST_LOG      => "hydra_builder=debug,info",
        },
    );

    _wait_for($self->{ua}, "$self->{base_url}/status/machines", sub {
        shift->decoded_content =~ /"hostname"/;
    }) or die "Timed out waiting for builder to register\n";
}

sub daemon_socket { return $_[0]->{daemon_sock}; }

sub pump_logs {
    my ($self) = @_;
    for my $key (keys %{$self->{procs}}) {
        my $h = $self->{procs}{$key}{harness};
        eval { $h->pump_nb };
        $self->_flush_harness($key);
    }
}

# Block until the queue runner reports no in-flight (active) builds for
# any of the given build ids. Bails out if the builder process dies.
sub wait_for_builds_to_finish {
    my ($self, @build_ids) = @_;
    my $ua = $self->{ua};
    my $base_url = $self->{base_url};

    my $timeout = 60 * scalar(@build_ids);
    $timeout = 60 if $timeout < 60;
    my $deadline = time() + $timeout;
    while (time() < $deadline) {
        $self->pump_logs;
        my $bl = $self->{procs}{builder}{harness};
        if ($bl && !$bl->pumpable) {
            $bl->finish;
            my $rc = $bl->result;
            die "builder exited unexpectedly (exit code $rc)\n";
        }

        my $all_done = 1;
        for my $bid (@build_ids) {
            my $resp = $ua->get("$base_url/status/build/$bid/active");
            if ($resp->decoded_content =~ /true/) {
                $all_done = 0;
                last;
            }
        }
        return 1 if $all_done;
        sleep 2;
    }
    die "timed out waiting for ad-hoc builds to finish\n";
}

sub run_cmd {
    my ($self, $timeout, @cmd) = @_;
    # The unix:// store ignores NIX_STORE_DIR for its logical store
    # path; the only way to make the client agree with the daemon is to
    # pass `?store=<dir>` as a URL parameter. The setting registered by
    # StoreConfigBase is named `store`, not `store-dir`.
    my $store_dir = $self->{ctx}{central}{nix_store_dir};
    my $remote = "unix://" . $self->{daemon_sock} . "?store=" . $store_dir;
    my %env = (
        %{ $self->{ctx}{central_env} },
        NIX_REMOTE => $remote,
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
        # Flush daemon-side logs so yath's event-timeout doesn't trigger
        # while we wait for the client to come back.
        $self->pump_logs;
        if ($err) {
            return (1, $cmd_out, $cmd_err . "\n[run_cmd: pump_nb error: $err]");
        }
        if (!$h->pumpable) {
            $h->finish;
            my $rc = scalar $h->result;
            $rc = 0 unless defined $rc;
            return ($rc, $cmd_out, $cmd_err);
        }
        select(undef, undef, undef, 0.5);
    }
    eval { $h->kill_kill };
    return (1, $cmd_out, $cmd_err . "\n[run_cmd: timed out after ${timeout}s]");
}

sub stop {
    my ($self) = @_;
    return if $self->{stopped};
    $self->{stopped} = 1;
    for my $key (qw(builder queue_runner drv_daemon upstream)) {
        my $entry = $self->{procs}{$key};
        next unless $entry;
        my $h = $entry->{harness};
        eval { $h->kill_kill };
        $self->_flush_harness($key, 1);
    }
}

sub DESTROY {
    my ($self) = @_;
    $self->stop;
}

1;
