use utf8;
package Hydra::Schema::Result::DerivationOutputs;

# Created by DBIx::Class::Schema::Loader
# DO NOT MODIFY THE FIRST PART OF THIS FILE

=head1 NAME

Hydra::Schema::Result::DerivationOutputs

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

=head1 TABLE: C<derivationoutputs>

=cut

__PACKAGE__->table("derivationoutputs");

=head1 ACCESSORS

=head2 drvpath

  data_type: 'text'
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
  "name",
  { data_type => "text", is_nullable => 0 },
  "path",
  { data_type => "text", is_nullable => 1 },
);

=head1 PRIMARY KEY

=over 4

=item * L</drvpath>

=item * L</name>

=back

=cut

__PACKAGE__->set_primary_key("drvpath", "name");

=head1 RELATIONS

=head2 buildstepoutputs

Type: has_many

Related object: L<Hydra::Schema::Result::BuildStepOutputs>

=cut

__PACKAGE__->has_many(
  "buildstepoutputs",
  "Hydra::Schema::Result::BuildStepOutputs",
  { "foreign.drvpath" => "self.drvpath", "foreign.name" => "self.name" },
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
  { is_deferrable => 0, on_delete => "CASCADE", on_update => "NO ACTION" },
);


# Created by DBIx::Class::Schema::Loader v0.07051 @ 2026-07-16 18:22:32
# DO NOT MODIFY THIS OR ANYTHING ABOVE! md5sum:CR9SJjFkirqyA/J00DQXPQ

my %hint = (
    columns => [
        'path'
    ],
);

sub json_hint {
    return \%hint;
}

1;
