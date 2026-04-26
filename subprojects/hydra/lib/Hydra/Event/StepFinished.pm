package Hydra::Event::StepFinished;

use strict;
use warnings;


sub parse :prototype(@) {
    unless (@_ == 3) {
        die "step_finished: payload takes exactly three arguments, but ", scalar(@_), " were given";
    }

    my ($drv_path, $attempt, $log_path) = @_;

    unless ($attempt =~ /^\d+$/) {
        die "step_finished: payload argument attempt should be an integer, but '", $attempt, "' was given"
    }

    return Hydra::Event::StepFinished->new($drv_path, int($attempt), $log_path);
}

sub new :prototype($$$$) {
    my ($self, $drv_path, $attempt, $log_path) = @_;

    $log_path = undef if $log_path eq "-";

    return bless {
        "drv_path" => $drv_path,
        "attempt" => $attempt,
        "log_path" => $log_path,
        "step" => undef,
    }, $self;
}

sub interestedIn {
    my ($self, $plugin) = @_;
    return int(defined($plugin->can('stepFinished')));
}

sub load {
    my ($self, $db) = @_;

    if (!defined($self->{"step"})) {
        $self->{"step"} = $db->resultset('BuildSteps')->find({
            drvpath => $self->{"drv_path"},
            attempt => $self->{"attempt"},
        }) or die "step not found for $self->{'drv_path'} attempt $self->{'attempt'}\n";
    }
}

sub execute {
    my ($self, $db, $plugin) = @_;

    $self->load($db);

    $plugin->stepFinished($self->{"step"}, $self->{"log_path"});

    return 1;
}

1;
