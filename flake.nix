{
  description = "Ferry development shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      forAllSystems = nixpkgs.lib.genAttrs [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          # mkShell's stdenv provides the C compiler and linker. Rust comes from
          # rustup rather than nixpkgs so stable, the MSRV and nightly are all
          # available, as in CI.
          default = pkgs.mkShell {
            packages = [
              pkgs.cargo-deny
              pkgs.git
              pkgs.hyperfine
              pkgs.jq
              pkgs.mdbook
              pkgs.rustup
              pkgs.typos
            ];
          };
        }
      );
    };
}
