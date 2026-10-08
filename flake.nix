{
  description = "FormalMusic: a YouTube Music client for Linux, on kopuzd";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # The daemon FormalMusic plays through, at the rev Cargo.toml pins
    # kopuz-client to; the two move together. Its own flake packages the
    # Dioxus app and not kopuzd, so kopuzd is built from source here.
    kopuz = {
      url = "github:FormalSnake/kopuz/2b9f8a657f8e8cc79e0a9144dd19464f6a304442";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, kopuz }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      linuxSystems = [ "x86_64-linux" "aarch64-linux" ];
      overlay = final: prev: {
        kopuzd = final.callPackage ./nix/kopuzd.nix { inherit kopuz; };
        formalmusic = final.callPackage ./nix/package.nix { };
      };
      pkgsFor = system: import nixpkgs { inherit system; overlays = [ overlay ]; };
      forAll = f: nixpkgs.lib.genAttrs systems (system: f (pkgsFor system));
    in
    {
      packages = nixpkgs.lib.genAttrs linuxSystems (system: rec {
        inherit (pkgsFor system) formalmusic kopuzd;
        default = formalmusic;
      });

      overlays.default = overlay;

      homeModules.default = import ./nix/hm-module.nix self;
      homeManagerModules.default = self.homeModules.default;

      devShells = forAll (pkgs:
        let
          # gpui-pre links libxkbcommon and freetype at build time and dlopens
          # wayland, vulkan, fontconfig and X11 at runtime.
          linuxLibs = with pkgs; [
            libxkbcommon
            wayland
            wayland-protocols
            vulkan-loader
            fontconfig.lib
            freetype
            libxcb
            libx11
            libxcursor
            libxi
            libxrandr
            libglvnd
          ];
        in
        {
          default = pkgs.mkShell {
            packages = [ pkgs.pkg-config pkgs.ffmpeg-headless ]
              ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux [ pkgs.kopuzd ]
              ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux ([ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt pkgs.fontconfig pkgs.fontconfig.dev pkgs.grim ] ++ linuxLibs);
            # The binary is built outside the Nix sandbox, so the dlopened
            # libraries go on LD_LIBRARY_PATH for both linking and running.
            shellHook = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
              export LD_LIBRARY_PATH=/run/opengl-driver/lib:${pkgs.lib.makeLibraryPath linuxLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
            '';
          };
        });
    };
}
