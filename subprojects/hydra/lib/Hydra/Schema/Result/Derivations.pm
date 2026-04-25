use utf8;
package Hydra::Schema::Result::Derivations;

# Created by DBIx::Class::Schema::Loader
# DO NOT MODIFY THE FIRST PART OF THIS FILE

=head1 NAME

Hydra::Schema::Result::Derivations

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

=head1 TABLE: C<derivations>

=cut

__PACKAGE__->table("derivations");

=head1 ACCESSORS

=head2 path

  data_type: 'text'
  is_nullable: 0

=cut

__PACKAGE__->add_columns("path", { data_type => "text", is_nullable => 0 });

=head1 PRIMARY KEY

=over 4

=item * L</path>

=back

=cut

__PACKAGE__->set_primary_key("path");

=head1 RELATIONS

=head2 buildstepcancreate

Type: might_have

Related object: L<Hydra::Schema::Result::BuildStepCanCreate>

=cut

__PACKAGE__->might_have(
  "buildstepcancreate",
  "Hydra::Schema::Result::BuildStepCanCreate",
  { "foreign.drvpath" => "self.path" },
  undef,
);

=head2 buildstepdeps_depdrvpaths

Type: has_many

Related object: L<Hydra::Schema::Result::BuildStepDeps>

=cut

__PACKAGE__->has_many(
  "buildstepdeps_depdrvpaths",
  "Hydra::Schema::Result::BuildStepDeps",
  { "foreign.depdrvpath" => "self.path" },
  undef,
);

=head2 buildstepdeps_drvpaths

Type: has_many

Related object: L<Hydra::Schema::Result::BuildStepDeps>

=cut

__PACKAGE__->has_many(
  "buildstepdeps_drvpaths",
  "Hydra::Schema::Result::BuildStepDeps",
  { "foreign.drvpath" => "self.path" },
  undef,
);

=head2 buildsteps

Type: has_many

Related object: L<Hydra::Schema::Result::BuildSteps>

=cut

__PACKAGE__->has_many(
  "buildsteps",
  "Hydra::Schema::Result::BuildSteps",
  { "foreign.drvpath" => "self.path" },
  undef,
);

=head2 buildstepshistoricals

Type: has_many

Related object: L<Hydra::Schema::Result::BuildStepsHistorical>

=cut

__PACKAGE__->has_many(
  "buildstepshistoricals",
  "Hydra::Schema::Result::BuildStepsHistorical",
  { "foreign.drvpath" => "self.path" },
  undef,
);

=head2 dep_derivations

Type: many_to_many

Composing rels: L</buildstepdeps_drvpaths> -> dep_derivation

=cut

__PACKAGE__->many_to_many("dep_derivations", "buildstepdeps_drvpaths", "dep_derivation");

=head2 derivations

Type: many_to_many

Composing rels: L</buildstepdeps_depdrvpaths> -> derivation

=cut

__PACKAGE__->many_to_many("derivations", "buildstepdeps_depdrvpaths", "derivation");


# Created by DBIx::Class::Schema::Loader v0.07051 @ 2026-04-26 16:52:15
# DO NOT MODIFY THIS OR ANYTHING ABOVE! md5sum:3Iv+w9qJeiUUnlKzjuFIow


# You can replace this text with custom code or comments, and it will be preserved on regeneration
1;
