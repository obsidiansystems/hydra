use utf8;
package Hydra::Schema::Result::BuildStepCanCreate;

# Created by DBIx::Class::Schema::Loader
# DO NOT MODIFY THE FIRST PART OF THIS FILE

=head1 NAME

Hydra::Schema::Result::BuildStepCanCreate

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

=head1 TABLE: C<buildstepcancreate>

=cut

__PACKAGE__->table("buildstepcancreate");

=head1 ACCESSORS

=head2 drvpath

  data_type: 'text'
  is_foreign_key: 1
  is_nullable: 0

=head2 readytime

  data_type: 'integer'
  is_nullable: 0

=cut

__PACKAGE__->add_columns(
  "drvpath",
  { data_type => "text", is_foreign_key => 1, is_nullable => 0 },
  "readytime",
  { data_type => "integer", is_nullable => 0 },
);

=head1 PRIMARY KEY

=over 4

=item * L</drvpath>

=back

=cut

__PACKAGE__->set_primary_key("drvpath");

=head1 RELATIONS

=head2 drvpath

Type: belongs_to

Related object: L<Hydra::Schema::Result::Derivations>

=cut

__PACKAGE__->belongs_to(
  "drvpath",
  "Hydra::Schema::Result::Derivations",
  { path => "drvpath" },
  { is_deferrable => 0, on_delete => "CASCADE", on_update => "NO ACTION" },
);


# Created by DBIx::Class::Schema::Loader v0.07051 @ 2026-04-25 21:55:24
# DO NOT MODIFY THIS OR ANYTHING ABOVE! md5sum:MbzMfi46exEptG1l3mBAkA


# You can replace this text with custom code or comments, and it will be preserved on regeneration
1;
