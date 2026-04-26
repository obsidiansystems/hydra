package Hydra::Controller::BuildStep;

use utf8;
use strict;
use warnings;
use base 'Hydra::Base::Controller::REST';
use Hydra::Helper::CatalystUtils;
use Hydra::Helper::LogEndpoints;
use File::Basename;
use WWW::Form::UrlEncoded::PP qw();


# Canonical route: /step/{drvPath_basename}/{attempt}
sub stepByDrvPathChain :Chained('/') :PathPart('step') :CaptureArgs(2) {
    my ($self, $c, $drvPathBasename, $attempt) = @_;

    my $drvPath = "/nix/store/$drvPathBasename";

    my $step = $c->model('DB::BuildSteps')->find({
        drvpath => $drvPath,
        attempt => $attempt,
    });
    notFound($c, "Build step not found for $drvPath attempt $attempt.") if !defined $step;

    $c->stash->{step} = $step;
    $c->stash->{build} = $step->build;
    $c->stash->{job} = $step->build->job;
    $c->stash->{jobset} = $step->build->jobset;
    $c->stash->{project} = $step->build->project;
}


sub buildStepByDrv : Chained('stepByDrvPathChain') PathPart('') Args(0) {
    my ($self, $c) = @_;
    $c->stash->{template} = 'build-step.tt';
}


sub view_nixlog_by_drv : Chained('stepByDrvPathChain') PathPart('log') {
    my ($self, $c, $mode) = @_;

    my $drvPath = $c->stash->{step}->drvpath;
    my $log_uri = $c->uri_for($c->controller('Root')->action_for("log"), [WWW::Form::UrlEncoded::PP::url_encode(basename($drvPath))]);
    showLog($c, $mode, $log_uri);
}


# Legacy route: /build/{id}/step/{stepnr} -> redirect to canonical
sub buildStepChain :Chained('/build/buildChain') :PathPart('step') :CaptureArgs(1) {
    my ($self, $c, $stepnr) = @_;

    my $step = $c->stash->{build}->buildsteps->find({stepnr => $stepnr});
    notFound($c, "Build doesn't have a build step $stepnr.") if !defined $step;

    $c->stash->{step} = $step;
}


sub buildStep : Chained('buildStepChain') PathPart('') Args(0) {
    my ($self, $c) = @_;

    my $step = $c->stash->{step};
    $c->res->redirect($c->uri_for('/step', basename($step->drvpath), $step->attempt), 301);
}


sub view_nixlog : Chained('buildStepChain') PathPart('log') {
    my ($self, $c, $mode) = @_;

    my $step = $c->stash->{step};
    my @path = ('/step', basename($step->drvpath), $step->attempt, 'log');
    push @path, $mode if defined $mode;
    $c->res->redirect($c->uri_for(@path), 301);
}


1;
