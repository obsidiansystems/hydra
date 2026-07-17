package Hydra::Schema::ResultSet::Builds;

use strict;
use utf8;
use warnings;

use parent 'DBIx::Class::ResultSet';

# A build is finished iff its stopTime is set (a check constraint ties
# this to the fulfilledBy/residual-status invariant); there is no
# stored "finished" flag. These chainable helpers are the one place
# that predicate is spelled in DBIC terms.

sub unfinished {
    my ($self) = @_;
    my $me = $self->current_source_alias;
    return $self->search({ "$me.stoptime" => undef });
}

sub finished {
    my ($self) = @_;
    my $me = $self->current_source_alias;
    return $self->search({ "$me.stoptime" => { '!=', undef } });
}

# Finished successfully in the legacy sense (buildstatus = 0): the
# fulfilling step succeeded and there is no residual status (a residual
# of 0, failure-with-output, also has a succeeding step but is a
# failure).
sub succeeded {
    my ($self) = @_;
    my $me = $self->current_source_alias;
    return $self->search(
        {
            "buildstep.status" => 0,
            "$me.buildstatus" => undef,
        },
        { join => "buildstep" },
    );
}

# Finished unsuccessfully in the legacy sense (buildstatus <> 0).
sub failed {
    my ($self) = @_;
    my $me = $self->current_source_alias;
    return $self->search(
        {
            "$me.stoptime" => { '!=', undef },
            -or => [
                "$me.buildstatus" => { '!=', undef },
                "buildstep.status" => { '!=', 0 },
            ],
        },
        { join => "buildstep" },
    );
}

# Builds whose legacy status is aborted (3), cancelled (4) or
# unsupported (9): the kinds "restart aborted" acts on. Either the
# residual status says so, or an aborted/cancelled/unsupported step
# fulfilled the build.
sub aborted {
    my ($self) = @_;
    my $me = $self->current_source_alias;
    return $self->search(
        {
            -or => [
                "$me.buildstatus" => { -in => [1, 2, 3] },
                -and => [
                    "$me.buildstatus" => undef,
                    "buildstep.status" => { -in => [3, 4, 9] },
                ],
            ],
        },
        { join => "buildstep" },
    );
}

1;
