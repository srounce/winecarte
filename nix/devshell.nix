{ pkgs, system, flake, ... }:
pkgs.mkShell {
  # Add build dependencies
  packages = [
    (flake.lib.mkRustToolchain system)
  ];

  # Add environment variables
  env = { };

  # Load custom bash code
  shellHook = ''

  '';
}
