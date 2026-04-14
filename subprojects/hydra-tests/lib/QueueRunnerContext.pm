use warnings;
use strict;

package QueueRunnerContext;
use File::Path qw(make_path);
use IO::Socket::IP;
use IPC::Run;
use LWP::UserAgent;
use POSIX qw(dup2);
use Hydra::Config;
our @ISA = qw(Exporter);
our @EXPORT = qw(
    start_builder
    start_queue_runner
    wait_for_builds
    wait_for_socket
    wait_for_url
);

sub wait_for_url {
    my ($ua, $url, $check) = @_;
    for my $i (1..30) {
        my $resp = $ua->get($url);
        if ($resp->is_success) {
            return 1 if !$check || $check->($resp);
        }
        select(undef, undef, undef, 0.5);
    }
    return 0;
}

sub wait_for_socket {
    my ($path) = @_;
    for my $i (1..60) {
        return 1 if -S $path;
        select(undef, undef, undef, 0.5);
    }
    return 0;
}

# Start a nix daemon for the given store config and register it with $pg.
sub start_nix_daemon {
    my ($pg, $key, $store) = @_;
    make_path($store->{nix_state_dir});

    $pg->spawn($key, ["nix-daemon"], env => {
        NIX_REMOTE             => $store->{nix_store_uri},
        NIX_STORE_DIR          => $store->{nix_store_dir},
        NIX_STATE_DIR          => $store->{nix_state_dir},
        NIX_CONF_DIR           => $store->{nix_conf_dir},
        NIX_DAEMON_SOCKET_PATH => $store->{nix_daemon_socket_path},
        NIX_CONFIG             => "trusted-users = *",
    });
    my $socket = $store->{nix_daemon_socket_path};
    for (1..50) {
        last if -S $socket;
        select(undef, undef, undef, 0.1);
    }
    -S $socket or die "nix-daemon did not start: $socket\n";
}

# Start a queue runner process using systemd socket activation and
# register it (plus its nix-daemon) with $pg.
#
# Returns ($rest_url, $grpc_addr).
#
# Options:
#   rust_log: RUST_LOG value, default "error".
#   queue_monitor_loop: 1 to leave the queue-monitor-loop running so
#     the queue runner picks up new Builds rows on its own (the
#     drv-daemon's ad-hoc flow needs this). Default 0 (disabled), which
#     matches QueueRunnerBuildOne's manual /build_one driver.
sub start_queue_runner {
    my ($pg, $ctx, %opts) = @_;
    ref $ctx eq 'HydraTestContext' or die "start_queue_runner requires a HydraTestContext\n";

    start_nix_daemon($pg, "qr-nix-daemon", $ctx->{central});

    my $config_dir = $ENV{T2_HARNESS_TEMP_DIR}
        // $ctx->{central}{hydra_data};
    my $config_file = "$config_dir/qr-config.toml";

    # Read store settings from the Hydra config file.
    my $hydra_config_file = $ctx->{central}{hydra_config_file};
    my $hydra_config = ($hydra_config_file && -f $hydra_config_file)
        ? Hydra::Config::loadConfig($hydra_config_file) : {};
    my $dest_store_uri = $hydra_config->{store_uri} // "";
    my $use_substitutes = $hydra_config->{'use-substitutes'} // "";

    # Write the TOML config for the queue runner.
    {
        my $db_url = $ctx->{central}{hydra_database_url};
        open(my $fh, '>', $config_file) or die "Cannot write $config_file: $!\n";
        print $fh "dbUrl = \"$db_url\"\n";
        print $fh "hydraDataDir = \"$config_dir/data\"\n";
        print $fh "remoteStoreAddr = [\"$dest_store_uri\"]\n" if $dest_store_uri ne "";
        print $fh "useSubstitutes = true\n" if $use_substitutes eq "1";
        close($fh);
    }

    # Bind TCP sockets for both servers (port 0 = OS picks a free port).
    my $rest_sock = IO::Socket::IP->new(
        LocalAddr => '::',
        LocalPort => 0,
        Proto     => 'tcp',
        Listen    => 128,
        ReuseAddr => 1,
        V6Only    => 0,
    ) or die "Cannot bind REST socket: $!\n";

    my $grpc_sock = IO::Socket::IP->new(
        LocalAddr => '::',
        LocalPort => 0,
        Proto     => 'tcp',
        Listen    => 128,
        ReuseAddr => 1,
        V6Only    => 0,
    ) or die "Cannot bind gRPC socket: $!\n";

    my $rest_port = $rest_sock->sockport;
    my $grpc_port = $grpc_sock->sockport;

    # The systemd socket activation protocol passes fds starting at 3.
    # We need to place our sockets at fd 3 and fd 4 in the child process.
    # IPC::Run's init callback runs in the child after fork.
    my $rest_fd = fileno($rest_sock);
    my $grpc_fd = fileno($grpc_sock);

    my @args = (
        "hydra-queue-runner",
        "--config-path", $config_file,
        "--rest-bind", "-",
        "--grpc-bind", "-",
    );
    push @args, "--disable-queue-monitor-loop" unless $opts{queue_monitor_loop};

    # Start the queue runner, connecting to the nix daemon via unix://.
    # We use IPC::Run directly (not $pg->spawn) because we need the
    # init callback to dup2 the socket fds into place.
    my ($qr_in, $qr_out, $qr_err) = ("", "", "");
    my $qr_harness;
    {
        local @ENV{keys %{$ctx->{central_env}}} = values %{$ctx->{central_env}};
        local $ENV{NIX_REMOTE} = $ctx->{central}{nix_daemon_uri};
        local $ENV{RUST_LOG} = $opts{rust_log} // "error";
        local $ENV{NO_COLOR} = "1";
        local $ENV{LISTEN_FDS} = "2";
        local $ENV{LISTEN_FDNAMES} = "rest:grpc";
        # Don't set LISTEN_PID — listenfd skips the PID check when it's unset.
        delete $ENV{LISTEN_PID};
        $qr_harness = IPC::Run::start(
            \@args,
            \$qr_in, \$qr_out, \$qr_err,
            init => sub {
                # In the child: place sockets at fd 3 and 4.
                POSIX::dup2($rest_fd, 3) or die "dup2 rest to fd 3: $!";
                POSIX::dup2($grpc_fd, 4) or die "dup2 grpc to fd 4: $!";
                # Close originals if they aren't already 3 or 4.
                POSIX::close($rest_fd) if $rest_fd != 3 && $rest_fd != 4;
                POSIX::close($grpc_fd) if $grpc_fd != 3 && $grpc_fd != 4;
            },
        );
    }
    $pg->register("queue-runner", $qr_harness, \$qr_out, \$qr_err);

    # Close our copies of the sockets (child has its own).
    close($rest_sock);
    close($grpc_sock);

    my $rest_url = "http://[::1]:$rest_port";
    my $grpc_addr = "[::1]:$grpc_port";

    return ($rest_url, $grpc_addr);
}

# Start a hydra-builder against an already-running queue runner
# and register it with $pg as "builder".
sub start_builder {
    my ($pg, $ctx, $grpc_addr, %opts) = @_;
    ref $ctx eq 'HydraTestContext' or die "start_builder requires a HydraTestContext\n";

    $pg->spawn("builder",
        ["hydra-builder", "--gateway-endpoint", "http://$grpc_addr"],
        env => {
            NIX_REMOTE    => $ctx->{builder}{nix_store_uri},
            NIX_CONF_DIR  => $ctx->{builder}{nix_conf_dir},
            NIX_STATE_DIR => $ctx->{builder}{nix_state_dir},
            # TODO: hydra-builder reads NIX_STORE_DIR to report its store
            # dir to the queue runner; should use the store URI instead.
            NIX_STORE_DIR => $ctx->{builder}{nix_store_dir},
            RUST_LOG      => $opts{rust_log} // "error",
        },
    );
}

# Poll the queue runner REST API until all given build IDs are no longer
# active. Calls $pg->pump_logs each iteration so test output stays visible.
#
# Args: ($ua, $base_url, $process_group, @build_ids)
# Dies on timeout or if the builder process exits unexpectedly.
sub wait_for_builds {
    my ($ua, $base_url, $pg, @build_ids) = @_;
    my $timeout = 60 * scalar(@build_ids);
    $timeout = 60 if $timeout < 60;
    my $deadline = time() + $timeout;
    while (time() < $deadline) {
        $pg->pump_logs;
        my $bl = $pg->harness("builder");
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
    die "timed out waiting for builds to finish\n";
}

1;
