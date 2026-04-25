use utf8;
package Hydra::Schema::Result::BuildStepDeps;

# Created by DBIx::Class::Schema::Loader
# DO NOT MODIFY THE FIRST PART OF THIS FILE

=head1 NAME

Hydra::Schema::Result::BuildStepDeps

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

=head1 TABLE: C<buildstepdeps>

=cut

__PACKAGE__->table("buildstepdeps");

=head1 ACCESSORS

=head2 drvpath

  data_type: 'text'
  is_foreign_key: 1
  is_nullable: 0

=head2 depdrvpath

  data_type: 'text'
  is_foreign_key: 1
  is_nullable: 0

=cut

__PACKAGE__->add_columns(
  "drvpath",
  { data_type => "text", is_foreign_key => 1, is_nullable => 0 },
  "depdrvpath",
  { data_type => "text", is_foreign_key => 1, is_nullable => 0 },
);

=head1 PRIMARY KEY

=over 4

=item * L</drvpath>

=item * L</depdrvpath>

=back

=cut

__PACKAGE__->set_primary_key("drvpath", "depdrvpath");

=head1 RELATIONS

=head2 dep_derivation

Type: belongs_to

Related object: L<Hydra::Schema::Result::Derivations>

=cut

__PACKAGE__->belongs_to(
  "dep_derivation",
  "Hydra::Schema::Result::Derivations",
  { path => "depdrvpath" },
  { is_deferrable => 0, on_delete => "CASCADE", on_update => "NO ACTION" },
);

=head2 derivation

Type: belongs_to

Related object: L<Hydra::Schema::Result::Derivations>

=cut

__PACKAGE__->belongs_to(
  "derivation",
  "Hydra::Schema::Result::Derivations",
  { path => "drvpath" },
  { is_deferrable => 0, on_delete => "CASCADE", on_update => "NO ACTION" },
);


# Created by DBIx::Class::Schema::Loader v0.07051 @ 2026-04-25 21:56:16
# DO NOT MODIFY THIS OR ANYTHING ABOVE! md5sum:MLck6M5afVzPLD6JUx7SSg


# You can replace this text with custom code or comments, and it will be preserved on regeneration
1;
