{ pkgs, system, flake, ... }:
let
  mingw = pkgs.pkgsCross.mingwW64;
in
pkgs.mkShell {
  packages = [
    (flake.lib.mkRustToolchain system)
    pkgs.just
    mingw.stdenv.cc
  ];

  depsTargetTarget = [
    mingw.windows.pthreads
  ];
}
