{
  pkgs,
  system,
  flake,
  ...
}:
let
  mingw = pkgs.pkgsCross.mingwW64;
  rustToolchain = flake.lib.mkRustToolchain system;
  rustPlatform = mingw.makeRustPlatform {
    cargo = rustToolchain;
    rustc = rustToolchain;
  };
  cargoToml = builtins.fromTOML (builtins.readFile ../../Cargo.toml);
  target = "x86_64-pc-windows-gnu";
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "wine2linux";
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

  buildInputs = [ mingw.windows.pthreads ];

  doCheck = false;

  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin
    install -m 755 target/${target}/release/wine2linux.exe $out/bin/
    runHook postInstall
  '';
})
