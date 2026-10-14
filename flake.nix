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
    # The UI toolkit, built from source beside this repo (the app depends on
    # it by path). Its own flake also carries the home-manager module the
    # settings options plug into.
    nativedesktop = {
      url = "github:FormalSnake/NativeDesktop";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, yt-dlp, nativedesktop }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      linuxSystems = [ "x86_64-linux" "aarch64-linux" ];
      overlay = final: prev: {
        yt-dlp = prev.yt-dlp.overrideAttrs {
          version = "${yt-dlp.lastModifiedDate}-${yt-dlp.shortRev}";
          src = yt-dlp;
        };
        formalmusic = final.callPackage ./nix/package.nix { inherit nativedesktop; };
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

      homeModules.default = import ./nix/hm-module.nix { inherit self nativedesktop; };
      homeManagerModules.default = self.homeModules.default;

      devShells = forAll (pkgs:
        let
          # nd's GPUI backend links libxkbcommon and freetype at build time and
          # dlopens wayland, vulkan, fontconfig and X11 at runtime; cpal needs
          # alsa-lib.
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
            libxkbfile
            libgbm
            openssl
            alsa-lib
            libopus
          ];
        in
        {
          default = pkgs.mkShell {
            FORMALMUSIC_YTDLP_PYTHON = "${pkgs.formalmusic.ytdlpPython}/bin/formalmusic-ytdlp-python";
            # symphonia-adapter-libopus links the system libopus through pkg-config
            # on every platform, macOS included.
            packages = [ pkgs.yt-dlp pkgs.socat pkgs.pkg-config pkgs.libopus pkgs.ffmpeg-headless ]
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
