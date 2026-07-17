use utf8;
package Hydra::Schema::Result::Builds;

# Created by DBIx::Class::Schema::Loader
# DO NOT MODIFY THE FIRST PART OF THIS FILE

=head1 NAME

Hydra::Schema::Result::Builds

=cut

use strict;
use warnings;

use base 'DBIx::Class::Core';

=head1 COMPONENTS LOADED

=over 4

=item * L<Hydra::Component::ToJSON>

=back

=cut

__PACKAGE__->load_components("+Hydra::Component::ToJSON");

=head1 TABLE: C<builds>

=cut

__PACKAGE__->table("builds");

=head1 ACCESSORS

=head2 id

  data_type: 'integer'
  is_auto_increment: 1
  is_nullable: 0
  sequence: 'builds_id_seq'

=head2 timestamp

  data_type: 'integer'
  is_nullable: 0

=head2 jobset_id

  data_type: 'integer'
  is_foreign_key: 1
  is_nullable: 0

=head2 job

  data_type: 'text'
  is_nullable: 0

=head2 nixname

  data_type: 'text'
  is_nullable: 1

=head2 description

  data_type: 'text'
  is_nullable: 1

=head2 drvpath

  data_type: 'text'
  is_foreign_key: 1
  is_nullable: 0

=head2 license

  data_type: 'text'
  is_nullable: 1

=head2 homepage

  data_type: 'text'
  is_nullable: 1

=head2 maintainers

  data_type: 'text'
  is_nullable: 1

=head2 maxsilent

  data_type: 'integer'
  default_value: 3600
  is_nullable: 1

=head2 timeout

  data_type: 'integer'
  default_value: 36000
  is_nullable: 1

=head2 ischannel

  data_type: 'integer'
  default_value: 0
  is_nullable: 0

=head2 iscurrent

  data_type: 'integer'
  default_value: 0
  is_nullable: 1

=head2 priority

  data_type: 'integer'
  default_value: 0
  is_nullable: 0

=head2 globalpriority

  data_type: 'integer'
  default_value: 0
  is_nullable: 0

=head2 stoptime

  data_type: 'integer'
  is_nullable: 1

=head2 fulfilledbydrvpath

  data_type: 'text'
  is_foreign_key: 1
  is_nullable: 1

=head2 fulfilledbyattempt

  data_type: 'integer'
  is_foreign_key: 1
  is_nullable: 1

=head2 buildstatus

  data_type: 'integer'
  is_nullable: 1

=head2 keep

  data_type: 'integer'
  default_value: 0
  is_nullable: 0

=head2 notificationpendingsince

  data_type: 'integer'
  is_nullable: 1

=cut

__PACKAGE__->add_columns(
  "id",
  {
    data_type         => "integer",
    is_auto_increment => 1,
    is_nullable       => 0,
    sequence          => "builds_id_seq",
  },
  "timestamp",
  { data_type => "integer", is_nullable => 0 },
  "jobset_id",
  { data_type => "integer", is_foreign_key => 1, is_nullable => 0 },
  "job",
  { data_type => "text", is_nullable => 0 },
  "nixname",
  { data_type => "text", is_nullable => 1 },
  "description",
  { data_type => "text", is_nullable => 1 },
  "drvpath",
  { data_type => "text", is_foreign_key => 1, is_nullable => 0 },
  "license",
  { data_type => "text", is_nullable => 1 },
  "homepage",
  { data_type => "text", is_nullable => 1 },
  "maintainers",
  { data_type => "text", is_nullable => 1 },
  "maxsilent",
  { data_type => "integer", default_value => 3600, is_nullable => 1 },
  "timeout",
  { data_type => "integer", default_value => 36000, is_nullable => 1 },
  "ischannel",
  { data_type => "integer", default_value => 0, is_nullable => 0 },
  "iscurrent",
  { data_type => "integer", default_value => 0, is_nullable => 1 },
  "priority",
  { data_type => "integer", default_value => 0, is_nullable => 0 },
  "globalpriority",
  { data_type => "integer", default_value => 0, is_nullable => 0 },
  "stoptime",
  { data_type => "integer", is_nullable => 1 },
  "fulfilledbydrvpath",
  { data_type => "text", is_foreign_key => 1, is_nullable => 1 },
  "fulfilledbyattempt",
  { data_type => "integer", is_foreign_key => 1, is_nullable => 1 },
  "buildstatus",
  { data_type => "integer", is_nullable => 1 },
  "keep",
  { data_type => "integer", default_value => 0, is_nullable => 0 },
  "notificationpendingsince",
  { data_type => "integer", is_nullable => 1 },
);

=head1 PRIMARY KEY

=over 4

=item * L</id>

=back

=cut

__PACKAGE__->set_primary_key("id");

=head1 RELATIONS

=head2 aggregateconstituents_aggregates

Type: has_many

Related object: L<Hydra::Schema::Result::AggregateConstituents>

=cut

__PACKAGE__->has_many(
  "aggregateconstituents_aggregates",
  "Hydra::Schema::Result::AggregateConstituents",
  { "foreign.aggregate" => "self.id" },
  undef,
);

=head2 aggregateconstituents_constituents

Type: has_many

Related object: L<Hydra::Schema::Result::AggregateConstituents>

=cut

__PACKAGE__->has_many(
  "aggregateconstituents_constituents",
  "Hydra::Schema::Result::AggregateConstituents",
  { "foreign.constituent" => "self.id" },
  undef,
);

=head2 buildinputs_builds

Type: has_many

Related object: L<Hydra::Schema::Result::BuildInputs>

=cut

__PACKAGE__->has_many(
  "buildinputs_builds",
  "Hydra::Schema::Result::BuildInputs",
  { "foreign.build" => "self.id" },
  undef,
);

=head2 buildinputs_dependencies

Type: has_many

Related object: L<Hydra::Schema::Result::BuildInputs>

=cut

__PACKAGE__->has_many(
  "buildinputs_dependencies",
  "Hydra::Schema::Result::BuildInputs",
  { "foreign.dependency" => "self.id" },
  undef,
);

=head2 buildmetrics

Type: has_many

Related object: L<Hydra::Schema::Result::BuildMetrics>

=cut

__PACKAGE__->has_many(
  "buildmetrics",
  "Hydra::Schema::Result::BuildMetrics",
  { "foreign.build" => "self.id" },
  undef,
);

=head2 buildproducts

Type: has_many

Related object: L<Hydra::Schema::Result::BuildProducts>

=cut

__PACKAGE__->has_many(
  "buildproducts",
  "Hydra::Schema::Result::BuildProducts",
  { "foreign.build" => "self.id" },
  undef,
);

=head2 buildstep

Type: belongs_to

Related object: L<Hydra::Schema::Result::BuildSteps>

=cut

__PACKAGE__->belongs_to(
  "buildstep",
  "Hydra::Schema::Result::BuildSteps",
  { attempt => "fulfilledbyattempt", drvpath => "fulfilledbydrvpath" },
  {
    is_deferrable => 0,
    join_type     => "LEFT",
    on_delete     => "NO ACTION",
    on_update     => "NO ACTION",
  },
);

=head2 buildsteps

Type: has_many

Related object: L<Hydra::Schema::Result::BuildSteps>

=cut

__PACKAGE__->has_many(
  "buildsteps",
  "Hydra::Schema::Result::BuildSteps",
  { "foreign.build" => "self.id" },
  undef,
);

=head2 buildsteps_propagatedfroms

Type: has_many

Related object: L<Hydra::Schema::Result::BuildSteps>

=cut

__PACKAGE__->has_many(
  "buildsteps_propagatedfroms",
  "Hydra::Schema::Result::BuildSteps",
  { "foreign.propagatedfrom" => "self.id" },
  undef,
);

=head2 derivation

Type: belongs_to

Related object: L<Hydra::Schema::Result::Derivations>

=cut

__PACKAGE__->belongs_to(
  "derivation",
  "Hydra::Schema::Result::Derivations",
  { path => "drvpath" },
  { is_deferrable => 0, on_delete => "NO ACTION", on_update => "NO ACTION" },
);

=head2 jobset

Type: belongs_to

Related object: L<Hydra::Schema::Result::Jobsets>

=cut

__PACKAGE__->belongs_to(
  "jobset",
  "Hydra::Schema::Result::Jobsets",
  { id => "jobset_id" },
  { is_deferrable => 0, on_delete => "CASCADE", on_update => "NO ACTION" },
);

=head2 jobsetevalinputs

Type: has_many

Related object: L<Hydra::Schema::Result::JobsetEvalInputs>

=cut

__PACKAGE__->has_many(
  "jobsetevalinputs",
  "Hydra::Schema::Result::JobsetEvalInputs",
  { "foreign.dependency" => "self.id" },
  undef,
);

=head2 jobsetevalmembers

Type: has_many

Related object: L<Hydra::Schema::Result::JobsetEvalMembers>

=cut

__PACKAGE__->has_many(
  "jobsetevalmembers",
  "Hydra::Schema::Result::JobsetEvalMembers",
  { "foreign.build" => "self.id" },
  undef,
);

=head2 runcommandlogs

Type: has_many

Related object: L<Hydra::Schema::Result::RunCommandLogs>

=cut

__PACKAGE__->has_many(
  "runcommandlogs",
  "Hydra::Schema::Result::RunCommandLogs",
  { "foreign.build_id" => "self.id" },
  undef,
);

=head2 aggregates

Type: many_to_many

Composing rels: L</aggregateconstituents_constituents> -> aggregate

=cut

__PACKAGE__->many_to_many(
  "aggregates",
  "aggregateconstituents_constituents",
  "aggregate",
);

=head2 constituents

Type: many_to_many

Composing rels: L</aggregateconstituents_aggregates> -> constituent

=cut

__PACKAGE__->many_to_many(
  "constituents",
  "aggregateconstituents_aggregates",
  "constituent",
);


# Created by DBIx::Class::Schema::Loader v0.07051 @ 2026-07-17 02:02:46
# DO NOT MODIFY THIS OR ANYTHING ABOVE! md5sum:DBXfe3taSmsptK4IUxyp3Q

__PACKAGE__->has_many(
  "dependents",
  "Hydra::Schema::Result::BuildInputs",
  { "foreign.dependency" => "self.id" },
);

__PACKAGE__->many_to_many(dependentBuilds => 'dependents', 'build');

__PACKAGE__->has_many(
  "inputs",
  "Hydra::Schema::Result::BuildInputs",
  { "foreign.build" => "self.id" },
);

__PACKAGE__->has_one(
  "actualBuildStep",
  "Hydra::Schema::Result::BuildSteps",
  { 'foreign.drvpath' => 'self.drvpath'
  , 'foreign.build' => 'self.id'
  },
);

__PACKAGE__->many_to_many("jobsetevals", "jobsetevalmembers", "eval");

__PACKAGE__->many_to_many("constituents_", "aggregateconstituents_aggregates", "constituent");

# The system column moved to Derivations; keep the old accessor working.
sub system {
    my ($self) = @_;
    return $self->derivation->system;
}

# Clearer alias for the loader-named `buildstep` relation: the attempt
# whose completion finished this build.
sub fulfilling_step {
    my ($self) = @_;
    return $self->buildstep;
}

# Compatibility accessors for the outcome columns that moved onto (or
# are now derived from) the fulfilling step. These keep row-object
# readers working; *queries* on the old columns must join instead.

sub finished {
    my ($self) = @_;
    return defined $self->get_column('stoptime') ? 1 : 0;
}

# The raw residual status column (see hydra.sql for its codes).
sub residual_buildstatus {
    my ($self) = @_;
    return $self->get_column('buildstatus');
}

# Override the column accessor: every existing reader of
# $build->buildstatus expects the legacy vocabulary (0 = succeeded,
# 1 = failed, 2 = dep-failed, ...), so derive it from the residual
# status and the fulfilling step. Undef while unfinished. Writes go
# through update({buildstatus => ...}) and store the residual codes.
sub buildstatus {
    my ($self) = @_;
    my $residual = $self->get_column('buildstatus');
    if (defined $residual) {
        return 6 if $residual == 0; # failure with output
        return 4 if $residual == 1; # cancelled
        return 3 if $residual == 2; # aborted before any attempt
        return 9 if $residual == 3; # unsupported system type
    }
    my $step = $self->fulfilling_step;
    return undef unless defined $step;
    # Fulfilled by a step for another derivation: a dependency failed.
    return 2 if $step->get_column('drvpath') ne $self->get_column('drvpath');
    return $step->status;
}

# Kept as an explicit alias for new code.
sub legacy_buildstatus {
    my ($self) = @_;
    return $self->buildstatus;
}

# stoptime is a real column again (when the build reached its terminal
# state); only starttime is derived, from the fulfilling attempt.
sub starttime {
    my ($self) = @_;
    my $step = $self->fulfilling_step;
    return defined $step ? $step->starttime : $self->get_column('stoptime');
}

sub size {
    my ($self) = @_;
    my $step = $self->fulfilling_step;
    return defined $step ? $step->size : undef;
}

sub closuresize {
    my ($self) = @_;
    my $step = $self->fulfilling_step;
    return defined $step ? $step->closuresize : undef;
}

sub releasename {
    my ($self) = @_;
    my $step = $self->fulfilling_step;
    return defined $step ? $step->releasename : undef;
}

sub iscachedbuild {
    my ($self) = @_;
    my $step = $self->fulfilling_step;
    return undef unless defined $step;
    # Cached iff the fulfilling attempt was dispatched on behalf of a
    # different build, or is a substitution (type 1): substituted
    # builds record the substitution step under their own id but were
    # historically counted as cached.
    return ($step->get_column('build') != $self->id || $step->type == 1) ? 1 : 0;
}

# Compatibility relation: BuildOutputs was dropped in favour of
# DerivationOutputs, which is keyed by drvPath rather than build id.
# Routing the old relation through drvPath keeps existing readers
# (`$build->buildoutputs`, `join => ["buildoutputs"]`, ...) working.
__PACKAGE__->has_many(
  "buildoutputs",
  "Hydra::Schema::Result::DerivationOutputs",
  { "foreign.drvpath" => "self.drvpath" },
  undef,
);

sub makeSource {
    my ($name, $query) = @_;
    my $source = __PACKAGE__->result_source_instance();
    my $new_source = $source->new($source);
    $new_source->source_name($name);
    $new_source->name(\ "($query)");
    Hydra::Schema->register_extra_source($name => $new_source);
}

sub makeQueries {
    my ($name, $constraint) = @_;

    my $activeJobs = "(select distinct jobset_id, job, d.system from Builds b join Derivations d on d.path = b.drvPath where isCurrent = 1 $constraint)";

    makeSource(
        "LatestSucceeded$name",
        <<QUERY
          select *
          from
            (select
               (select max(b.id) from builds b
                join Derivations d on d.path = b.drvPath
                join BuildSteps fs on fs.drvPath = b.fulfilledByDrvPath
                                  and fs.attempt = b.fulfilledByAttempt
                where
                  jobset_id = activeJobs.jobset_id
                  and job = activeJobs.job and d.system = activeJobs.system
                  and fs.status = 0 and b.buildStatus is null
               ) as id
             from $activeJobs as activeJobs
            ) as latest
          join Builds using (id)
QUERY
    );
}

makeQueries('', "");
makeQueries('ForProject', "and jobset_id in (select id from jobsets j where j.project = ?)");
makeQueries('ForJobset', "and jobset_id = ?");
makeQueries('ForJob', "and jobset_id = ? and job = ?");
makeQueries('ForJobName', "and jobset_id = (select id from jobsets j where j.project = ? and j.name = ?) and job = ?");

sub as_json {
  my ($self) = @_;

  # After #1093 merges this can become $self->jobset;
  # However, with ->jobset being a column on master
  # it seems DBIX gets a it confused.
  my ($jobset) = $self->search_related('jobset')->first;

  my $json = {
    id => $self->get_column('id'),
    finished => $self->finished,
    timestamp => $self->get_column('timestamp'),
    starttime => $self->starttime,
    stoptime => $self->stoptime,
    project => $jobset->get_column('project'),
    jobset => $jobset->name,
    job => $self->get_column('job'),
    nixname => $self->get_column('nixname'),
    system => $self->derivation->system,
    priority => $self->get_column('priority'),
    # The legacy status vocabulary, for API compatibility.
    buildstatus => $self->legacy_buildstatus,
    releasename => $self->releasename,
    drvpath => $self->get_column('drvpath'),
    jobsetevals => [ map { $_->id } $self->jobsetevals ],
    buildoutputs => { map { $_->name  => $_ } $self->derivation->derivationoutputs },
    buildproducts => { map { $_->productnr => $_ } $self->buildproducts },
    buildmetrics => { map { $_->name => $_ } $self->buildmetrics },
  };

  return $json;
}

sub project {
  my ($self) = @_;
  return $self->jobset->project;
}

1;
