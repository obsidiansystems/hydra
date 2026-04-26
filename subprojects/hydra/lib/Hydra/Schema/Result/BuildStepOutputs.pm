use utf8;
package Hydra::Schema::Result::BuildStepOutputs;

# Created by DBIx::Class::Schema::Loader
# DO NOT MODIFY THE FIRST PART OF THIS FILE

=head1 NAME

Hydra::Schema::Result::BuildStepOutputs

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

=head1 TABLE: C<buildstepoutputs>

=cut

__PACKAGE__->table("buildstepoutputs");

=head1 ACCESSORS

=head2 drvpath

  data_type: 'text'
  is_foreign_key: 1
  is_nullable: 0

=head2 attempt

  data_type: 'integer'
  is_foreign_key: 1
  is_nullable: 0

=head2 name

  data_type: 'text'
  is_nullable: 0

=head2 path

  data_type: 'text'
  is_nullable: 1

=cut

__PACKAGE__->add_columns(
  "drvpath",
  { data_type => "text", is_foreign_key => 1, is_nullable => 0 },
  "attempt",
  { data_type => "integer", is_foreign_key => 1, is_nullable => 0 },
  "name",
  { data_type => "text", is_nullable => 0 },
  "path",
  { data_type => "text", is_nullable => 1 },
);

=head1 PRIMARY KEY

=over 4

=item * L</drvpath>

=item * L</attempt>

=item * L</name>

=back

=cut

__PACKAGE__->set_primary_key("drvpath", "attempt", "name");

=head1 RELATIONS

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


# Created by DBIx::Class::Schema::Loader v0.07051 @ 2026-04-25 21:19:23
# DO NOT MODIFY THIS OR ANYTHING ABOVE! md5sum:ERMZNrSYz2lMLBuyMiZcuQ


# You can replace this text with custom code or comments, and it will be preserved on regeneration
1;
