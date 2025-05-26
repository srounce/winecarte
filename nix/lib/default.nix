{ inputs, ... }: {
  mkRustToolchain = system:
  let
    toolchainToml = (builtins.fromTOML (builtins.readFile ../../rust-toolchain.toml)).toolchain;
  in
  inputs.fenix.packages.${system}.stable.withComponents toolchainToml.components;
}
