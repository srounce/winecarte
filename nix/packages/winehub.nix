{
  pkgs,
  system,
  flake,
  ...
}:
let
  rustToolchain = flake.lib.mkRustToolchain system;
  rustPlatform = pkgs.makeRustPlatform {
    cargo = rustToolchain;
    rustc = rustToolchain;
  };
  cargoToml = builtins.fromTOML (builtins.readFile ../../Cargo.toml);
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "winehub";
  version = cargoToml.workspace.package.version;
  src = ../..;
  cargoBuildFlags = [
    "--package"
    finalAttrs.pname
  ];
  cargoTestFlags = [
    "--package"
    finalAttrs.pname
  ];
  cargoLock.lockFile = ../../Cargo.lock;
})
