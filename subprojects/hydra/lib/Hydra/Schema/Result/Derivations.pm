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

=head2 system

  data_type: 'text'
  is_nullable: 1

=cut

__PACKAGE__->add_columns(
  "path",
  { data_type => "text", is_nullable => 0 },
  "system",
  { data_type => "text", is_nullable => 1 },
);

=head1 PRIMARY KEY

=over 4

=item * L</path>

=back

=cut

__PACKAGE__->set_primary_key("path");

=head1 RELATIONS

=head2 builds

Type: has_many

Related object: L<Hydra::Schema::Result::Builds>

=cut

__PACKAGE__->has_many(
  "builds",
  "Hydra::Schema::Result::Builds",
  { "foreign.drvpath" => "self.path" },
  undef,
);

=head2 buildstepoutputs

Type: has_many

Related object: L<Hydra::Schema::Result::BuildStepOutputs>

=cut

__PACKAGE__->has_many(
  "buildstepoutputs",
  "Hydra::Schema::Result::BuildStepOutputs",
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

=head2 derivationoutputs

Type: has_many

Related object: L<Hydra::Schema::Result::DerivationOutputs>

=cut

__PACKAGE__->has_many(
  "derivationoutputs",
  "Hydra::Schema::Result::DerivationOutputs",
  { "foreign.drvpath" => "self.path" },
  undef,
);


# Created by DBIx::Class::Schema::Loader v0.07051 @ 2026-07-16 18:31:17
# DO NOT MODIFY THIS OR ANYTHING ABOVE! md5sum:Ju5f4aW8o8FrTLk9XJeuew


# You can replace this text with custom code or comments, and it will be preserved on regeneration
1;
