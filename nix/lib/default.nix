{ inputs, ... }: {
  mkRustToolchain = system:
  let
    toolchainToml = (fromTOML (builtins.readFile ../../rust-toolchain.toml)).toolchain;
  in
  inputs.fenix.packages.${system}.fromToolchainFile {
    file = ../../rust-toolchain.toml;
    sha256 = "sha256-gh/xTkxKHL4eiRXzWv8KP7vfjSk61Iq48x47BEDFgfk=";
  };
  # inputs.fenix.packages.${system}.stable.withComponents toolchainToml.components;
}
