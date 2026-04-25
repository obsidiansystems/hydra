use utf8;
package Hydra::Schema::Result::DerivationPaths;

# Created by DBIx::Class::Schema::Loader
# DO NOT MODIFY THE FIRST PART OF THIS FILE

=head1 NAME

Hydra::Schema::Result::DerivationPaths

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

=head1 TABLE: C<derivationpaths>

=cut

__PACKAGE__->table("derivationpaths");

=head1 ACCESSORS

=head2 drvpath

  data_type: 'text'
  is_nullable: 0

=cut

__PACKAGE__->add_columns("drvpath", { data_type => "text", is_nullable => 0 });

=head1 PRIMARY KEY

=over 4

=item * L</drvpath>

=back

=cut

__PACKAGE__->set_primary_key("drvpath");

=head1 RELATIONS

=head2 buildstepdeps_depdrvpaths

Type: has_many

Related object: L<Hydra::Schema::Result::BuildStepDeps>

=cut

__PACKAGE__->has_many(
  "buildstepdeps_depdrvpaths",
  "Hydra::Schema::Result::BuildStepDeps",
  { "foreign.depdrvpath" => "self.drvpath" },
  undef,
);

=head2 buildstepdeps_drvpaths

Type: has_many

Related object: L<Hydra::Schema::Result::BuildStepDeps>

=cut

__PACKAGE__->has_many(
  "buildstepdeps_drvpaths",
  "Hydra::Schema::Result::BuildStepDeps",
  { "foreign.drvpath" => "self.drvpath" },
  undef,
);

=head2 buildsteps

Type: has_many

Related object: L<Hydra::Schema::Result::BuildSteps>

=cut

__PACKAGE__->has_many(
  "buildsteps",
  "Hydra::Schema::Result::BuildSteps",
  { "foreign.drvpath" => "self.drvpath" },
  undef,
);

=head2 depdrvpaths

Type: many_to_many

Composing rels: L</buildstepdeps_drvpaths> -> depdrvpath

=cut

__PACKAGE__->many_to_many("depdrvpaths", "buildstepdeps_drvpaths", "depdrvpath");

=head2 drvpaths

Type: many_to_many

Composing rels: L</buildstepdeps_depdrvpaths> -> drvpath

=cut

__PACKAGE__->many_to_many("drvpaths", "buildstepdeps_depdrvpaths", "drvpath");


# Created by DBIx::Class::Schema::Loader v0.07051 @ 2026-04-13 22:02:50
# DO NOT MODIFY THIS OR ANYTHING ABOVE! md5sum:sL1QvEaqHe+W+tXsm1kpUg


# You can replace this text with custom code or comments, and it will be preserved on regeneration
1;
