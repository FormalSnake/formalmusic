{
  description = "FormalMusic: a YouTube Music client for Linux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Pinned to a yt-dlp release instead of nixpkgs' copy, which trails
    # releases by days. YouTube breaks extraction often enough that the weekly
    # maintenance run bumps this input on its own.
    yt-dlp = {
      url = "github:yt-dlp/yt-dlp/2026.08.19";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, yt-dlp }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      linuxSystems = [ "x86_64-linux" "aarch64-linux" ];
      overlay = final: prev: {
        yt-dlp = prev.yt-dlp.overrideAttrs {
          version = "${yt-dlp.lastModifiedDate}-${yt-dlp.shortRev}";
          src = yt-dlp;
        };
        formalmusic = final.callPackage ./nix/package.nix { };
      };
      pkgsFor = system: import nixpkgs { inherit system; overlays = [ overlay ]; };
      forAll = f: nixpkgs.lib.genAttrs systems (system: f (pkgsFor system));
    in
    {
      packages = nixpkgs.lib.genAttrs linuxSystems (system: rec {
        inherit (pkgsFor system) formalmusic yt-dlp;
        default = formalmusic;
      });

      overlays.default = overlay;

      homeModules.default = import ./nix/hm-module.nix self;
      homeManagerModules.default = self.homeModules.default;

      devShells = forAll (pkgs:
        let
          # gpui-pre links libxkbcommon and freetype at build time and dlopens
          # wayland, vulkan, fontconfig and X11 at runtime; cpal needs alsa-lib.
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
            alsa-lib
            libopus
          ];
        in
        {
          default = pkgs.mkShell {
            packages = [ pkgs.yt-dlp pkgs.socat ]
              ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux ([ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt pkgs.fontconfig pkgs.fontconfig.dev pkgs.grim pkgs.pkg-config ] ++ linuxLibs);
            # The binary is built outside the Nix sandbox, so the dlopened
            # libraries go on LD_LIBRARY_PATH for both linking and running.
            shellHook = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
              export LD_LIBRARY_PATH=/run/opengl-driver/lib:${pkgs.lib.makeLibraryPath linuxLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
            '';
          };
        });
    };
}
