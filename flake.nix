{
  description = "Herdr desktop app for terminals and coding agents";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    # Keep this revision in sync with the vendor/ghostty submodule.
    ghostty = {
      url = "github:ghostty-org/ghostty/291fd050b9e9c293c8adb6a140742649b7e42d3a";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      ghostty,
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      pkgsFor = system: import nixpkgs { inherit system; };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          herdr-app = pkgs.callPackage ./nix/package.nix { inherit ghostty; };
        in
        {
          inherit herdr-app;
          default = herdr-app;
        }
      );

      apps = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          sourceApp =
            name: command:
            let
              wrapper = pkgs.writeShellApplication {
                name = "herdr-${name}";
                runtimeInputs = [ pkgs.nix ];
                text = ''
                  if [[ ! -f Cargo.toml || ! -f flake.nix ]]; then
                    echo 'Run this command from the herdr-app checkout root.' >&2
                    exit 1
                  fi
                  exec nix develop --command ${command} "$@"
                '';
              };
            in
            {
              type = "app";
              program = "${wrapper}/bin/herdr-${name}";
              meta.description = "Run Herdr's ${name} development command";
            };
          app = {
            type = "app";
            program = "${self.packages.${system}.herdr-app}/bin/herdr-app";
            meta.description = "Herdr desktop app";
          };
        in
        {
          default = app;
          herdr-app = app;
          dev = sourceApp "dev" "cargo run --locked --";
          build = sourceApp "build" "just build";
          test = sourceApp "test" "cargo test --locked";
          check = sourceApp "check" "just check";
          fmt = sourceApp "fmt" "cargo fmt --package herdr-app --package ghostty-sys";
          fmt-check = sourceApp "fmt-check" "cargo fmt --package herdr-app --package ghostty-sys --check";
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
          package = self.packages.${system}.herdr-app;
        in
        {
          default = pkgs.mkShell {
            inputsFrom = [ package ];
            packages = with pkgs; [
              rustc
              cargo
              rustfmt
              clippy
              just
              bun
              git
              python3
            ];
            inherit (package) LIBCLANG_PATH;
            LD_LIBRARY_PATH = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux (
              pkgs.lib.makeLibraryPath package.runtimeDependencies
            );
            shellHook = ''
              export ZIG_GLOBAL_CACHE_DIR="''${XDG_CACHE_HOME:-$HOME/.cache}/herdr-app/${builtins.baseNameOf package.zigDeps}"
              mkdir -p "$ZIG_GLOBAL_CACHE_DIR"
              if [[ ! -e "$ZIG_GLOBAL_CACHE_DIR/p" ]]; then
                ln -s ${package.zigDeps} "$ZIG_GLOBAL_CACHE_DIR/p"
              fi
            '';
          };
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = pkgsFor system;
        in
        {
          inherit (self.packages.${system}) herdr-app;
          nix-format = pkgs.runCommand "herdr-nix-format" { nativeBuildInputs = [ pkgs.nixfmt ]; } ''
            nixfmt --check ${./flake.nix} ${./nix/package.nix}
            touch "$out"
          '';
        }
      );
      formatter = forAllSystems (
        system:
        (pkgsFor system).nixfmt-tree.override {
          settings.excludes = [ "vendor/**" ];
        }
      );
    };
}
