use utf8;
package Hydra::Schema::Result::BuildSteps;

# Created by DBIx::Class::Schema::Loader
# DO NOT MODIFY THE FIRST PART OF THIS FILE

=head1 NAME

Hydra::Schema::Result::BuildSteps

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

=head1 TABLE: C<buildsteps>

=cut

__PACKAGE__->table("buildsteps");

=head1 ACCESSORS

=head2 type

  data_type: 'integer'
  is_nullable: 0

=head2 drvpath

  data_type: 'text'
  is_foreign_key: 1
  is_nullable: 0

=head2 attempt

  data_type: 'integer'
  default_value: 0
  is_nullable: 0

=head2 busy

  data_type: 'integer'
  is_nullable: 0

=head2 status

  data_type: 'integer'
  is_nullable: 1

=head2 errormsg

  data_type: 'text'
  is_nullable: 1

=head2 starttime

  data_type: 'integer'
  is_nullable: 1

=head2 stoptime

  data_type: 'integer'
  is_nullable: 1

=head2 machine

  data_type: 'text'
  default_value: (empty string)
  is_nullable: 0

=head2 system

  data_type: 'text'
  is_nullable: 1

=head2 propagatedfrom

  data_type: 'integer'
  is_foreign_key: 1
  is_nullable: 1

=head2 overhead

  data_type: 'integer'
  is_nullable: 1

=head2 timesbuilt

  data_type: 'integer'
  is_nullable: 1

=head2 isnondeterministic

  data_type: 'boolean'
  is_nullable: 1

=cut

__PACKAGE__->add_columns(
  "type",
  { data_type => "integer", is_nullable => 0 },
  "drvpath",
  { data_type => "text", is_foreign_key => 1, is_nullable => 0 },
  "attempt",
  { data_type => "integer", default_value => 0, is_nullable => 0 },
  "busy",
  { data_type => "integer", is_nullable => 0 },
  "status",
  { data_type => "integer", is_nullable => 1 },
  "errormsg",
  { data_type => "text", is_nullable => 1 },
  "starttime",
  { data_type => "integer", is_nullable => 1 },
  "stoptime",
  { data_type => "integer", is_nullable => 1 },
  "machine",
  { data_type => "text", default_value => "", is_nullable => 0 },
  "system",
  { data_type => "text", is_nullable => 1 },
  "propagatedfrom",
  { data_type => "integer", is_foreign_key => 1, is_nullable => 1 },
  "overhead",
  { data_type => "integer", is_nullable => 1 },
  "timesbuilt",
  { data_type => "integer", is_nullable => 1 },
  "isnondeterministic",
  { data_type => "boolean", is_nullable => 1 },
);

=head1 PRIMARY KEY

=over 4

=item * L</drvpath>

=item * L</attempt>

=back

=cut

__PACKAGE__->set_primary_key("drvpath", "attempt");

=head1 RELATIONS

=head2 buildstepoutputs

Type: has_many

Related object: L<Hydra::Schema::Result::BuildStepOutputs>

=cut

__PACKAGE__->has_many(
  "buildstepoutputs",
  "Hydra::Schema::Result::BuildStepOutputs",
  {
    "foreign.attempt" => "self.attempt",
    "foreign.drvpath" => "self.drvpath",
  },
  undef,
);

=head2 buildstepshistorical

Type: might_have

Related object: L<Hydra::Schema::Result::BuildStepsHistorical>

=cut

__PACKAGE__->might_have(
  "buildstepshistorical",
  "Hydra::Schema::Result::BuildStepsHistorical",
  {
    "foreign.attempt" => "self.attempt",
    "foreign.drvpath" => "self.drvpath",
  },
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

=head2 propagatedfrom

Type: belongs_to

Related object: L<Hydra::Schema::Result::Builds>

=cut

__PACKAGE__->belongs_to(
  "propagatedfrom",
  "Hydra::Schema::Result::Builds",
  { id => "propagatedfrom" },
  {
    is_deferrable => 0,
    join_type     => "LEFT",
    on_delete     => "CASCADE",
    on_update     => "NO ACTION",
  },
);


# Created by DBIx::Class::Schema::Loader v0.07051 @ 2026-04-26 16:52:15
# DO NOT MODIFY THIS OR ANYTHING ABOVE! md5sum:MnyaF4rAuWP1bZ9vFNSE4w

my %hint = (
    columns => [
        "machine",
        "system",
        "drvpath",
        "attempt",
        "starttime",
    ],
);

sub json_hint {
    return \%hint;
}

1;
