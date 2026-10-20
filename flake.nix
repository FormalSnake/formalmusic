{
  description = "FormalMusic: a YouTube Music client for Linux";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # The daemon. Pinned to the release Cargo.toml's kopuz-* crates come from:
    # the wire changes with the app, so the client and the daemon move together.
    # Not following our nixpkgs keeps kopuz.cachix.org's builds usable.
    kopuz.url = "github:Kopuz-org/kopuz/v0.22.0";
    # The UI toolkit, built from source beside this repo (the app depends on
    # it by path). Its own flake also carries the home-manager module the
    # settings options plug into.
    nativedesktop = {
      url = "github:FormalSnake/NativeDesktop";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, kopuz, nativedesktop }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      linuxSystems = [ "x86_64-linux" "aarch64-linux" ];
      overlay = final: prev: {
        kopuz = kopuz.packages.${final.stdenv.hostPlatform.system}.default;
        formalmusic = final.callPackage ./nix/package.nix { inherit nativedesktop; };
      };
      pkgsFor = system: import nixpkgs { inherit system; overlays = [ overlay ]; };
      forAll = f: nixpkgs.lib.genAttrs systems (system: f (pkgsFor system));
    in
    {
      packages = nixpkgs.lib.genAttrs linuxSystems (system: rec {
        inherit (pkgsFor system) formalmusic kopuz;
        default = formalmusic;
      });

      overlays.default = overlay;

      homeModules.default = import ./nix/hm-module.nix { inherit self nativedesktop kopuz; };
      homeManagerModules.default = self.homeModules.default;

      devShells = forAll (pkgs:
        let
          # nd's GPUI backend links libxkbcommon and freetype at build time and
          # dlopens wayland, vulkan, fontconfig and X11 at runtime.
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
          ];
        in
        {
          default = pkgs.mkShell {
            # kopuzd for a dev box with no service running it; the app starts it when
            # nothing answers on the socket. grpcurl reads the daemon by hand.
            packages = [ pkgs.kopuz pkgs.grpcurl pkgs.pkg-config pkgs.ffmpeg-headless ]
              ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux ([ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.rustfmt pkgs.fontconfig pkgs.fontconfig.dev pkgs.grim pkgs.labwc pkgs.dbus ] ++ linuxLibs);
            # labwc lends the tier 3 tests' headless Hyprland a render node (Hyprland itself
            # is the system's).
            # The binary is built outside the Nix sandbox, so the dlopened
            # libraries go on LD_LIBRARY_PATH for both linking and running.
            shellHook = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux ''
              export LD_LIBRARY_PATH=/run/opengl-driver/lib:${pkgs.lib.makeLibraryPath linuxLibs}''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
            '';
          };
        });
    };
}
