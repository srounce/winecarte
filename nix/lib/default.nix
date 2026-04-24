{ inputs, ... }: {
  mkRustToolchain = system:
    inputs.fenix.packages.${system}.fromToolchainFile {
      file = ../../rust-toolchain.toml;
      sha256 = "sha256-gh/xTkxKHL4eiRXzWv8KP7vfjSk61Iq48x47BEDFgfk=";
    };
}
