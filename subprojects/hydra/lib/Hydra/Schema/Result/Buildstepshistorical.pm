use utf8;
package Hydra::Schema::Result::Buildstepshistorical;

# Created by DBIx::Class::Schema::Loader
# DO NOT MODIFY THE FIRST PART OF THIS FILE

=head1 NAME

Hydra::Schema::Result::Buildstepshistorical

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

=head1 TABLE: C<buildstepshistorical>

=cut

__PACKAGE__->table("buildstepshistorical");

=head1 ACCESSORS

=head2 drvpath

  data_type: 'text'
  is_foreign_key: 1
  is_nullable: 0

=head2 attempt

  data_type: 'integer'
  default_value: 0
  is_foreign_key: 1
  is_nullable: 0

=head2 build

  data_type: 'integer'
  is_foreign_key: 1
  is_nullable: 0

=head2 stepnr

  data_type: 'integer'
  is_nullable: 0

=cut

__PACKAGE__->add_columns(
  "drvpath",
  { data_type => "text", is_foreign_key => 1, is_nullable => 0 },
  "attempt",
  {
    data_type      => "integer",
    default_value  => 0,
    is_foreign_key => 1,
    is_nullable    => 0,
  },
  "build",
  { data_type => "integer", is_foreign_key => 1, is_nullable => 0 },
  "stepnr",
  { data_type => "integer", is_nullable => 0 },
);

=head1 PRIMARY KEY

=over 4

=item * L</drvpath>

=item * L</attempt>

=back

=cut

__PACKAGE__->set_primary_key("drvpath", "attempt");

=head1 UNIQUE CONSTRAINTS

=head2 C<buildstepshistorical_build_stepnr_key>

=over 4

=item * L</build>

=item * L</stepnr>

=back

=cut

__PACKAGE__->add_unique_constraint("buildstepshistorical_build_stepnr_key", ["build", "stepnr"]);

=head1 RELATIONS

=head2 build

Type: belongs_to

Related object: L<Hydra::Schema::Result::Builds>

=cut

__PACKAGE__->belongs_to(
  "build",
  "Hydra::Schema::Result::Builds",
  { id => "build" },
  { is_deferrable => 0, on_delete => "CASCADE", on_update => "NO ACTION" },
);

=head2 buildstep

Type: belongs_to

Related object: L<Hydra::Schema::Result::BuildSteps>

=cut

__PACKAGE__->belongs_to(
  "buildstep",
  "Hydra::Schema::Result::BuildSteps",
  { attempt => "attempt", drvpath => "drvpath" },
  { is_deferrable => 0, on_delete => "CASCADE", on_update => "NO ACTION" },
);

=head2 drvpath

Type: belongs_to

Related object: L<Hydra::Schema::Result::Derivations>

=cut

__PACKAGE__->belongs_to(
  "drvpath",
  "Hydra::Schema::Result::Derivations",
  { path => "drvpath" },
  { is_deferrable => 0, on_delete => "NO ACTION", on_update => "NO ACTION" },
);


# Created by DBIx::Class::Schema::Loader v0.07051 @ 2026-04-25 21:55:24
# DO NOT MODIFY THIS OR ANYTHING ABOVE! md5sum:oOe8ATPUpIF3AHulYjCZlQ


# You can replace this text with custom code or comments, and it will be preserved on regeneration
1;
