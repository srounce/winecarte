{ pkgs, system, flake, ... }:
pkgs.mkShell {
  # Add build dependencies
  packages = [
    (flake.lib.mkRustToolchain system)
  ] ++ (with pkgs.pkgsCross.mingwW64; [
    stdenv.cc
    windows.pthreads
  ]);

  # Add environment variables
  env = { };

  # Load custom bash code
  shellHook = ''

  '';
}
