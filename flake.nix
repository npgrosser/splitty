{
  description = "Splitty, a graphical terminal multiplexer";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
      # The published release `splitty-bin` installs (packaging/nix/update-release.sh).
      release = builtins.fromJSON (builtins.readFile ./packaging/nix/release.json);

      libghostty-vt = pkgs.callPackage ./packaging/nix/libghostty-vt.nix { };
      splitty-bin = pkgs.callPackage ./packaging/nix/bin.nix { inherit release; };
      splitty = pkgs.callPackage ./packaging/nix/source.nix {
        inherit release libghostty-vt;
        commit = self.shortRev or self.dirtyShortRev or "unknown";
      };
    in
    {
      packages.${system} = {
        inherit splitty splitty-bin libghostty-vt;
        default = splitty-bin;
      };

      # What a build from the checkout needs, with the terminal library set.
      devShells.${system}.default = pkgs.mkShell {
        inputsFrom = [ splitty ];
        packages = [
          pkgs.cargo
          pkgs.rustc
          pkgs.clippy
          pkgs.rustfmt
        ];
        GHOSTTY_VT_DIR = "${libghostty-vt}";
        LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
          pkgs.wayland
          pkgs.libxkbcommon
          pkgs.libglvnd
          pkgs.vulkan-loader
        ];
      };
    };
}
