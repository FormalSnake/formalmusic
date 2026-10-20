{ self, nativedesktop, kopuz }:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.formalmusic;
in
{
  # nativedesktop's module: `enable`, `package`, `theme` (written to theme.json; matugen
  # can own it instead) and `settings`, the typed options generated from the settings
  # schema (nix/settings-options.nix), rendered to config.toml. Keys set there show
  # read-only in the app's Settings window, and the window pushes the playback keys into
  # Kopuz's own settings.
  imports = [
    (import "${nativedesktop}/nix/hm-module.nix" {
      pname = "formalmusic";
      settingsOptions = import ./settings-options.nix;
      defaultPackage = system: self.packages.${system}.default;
    })
  ];

  options.programs.formalmusic.daemon = lib.mkOption {
    type = lib.types.bool;
    default = true;
    description = ''
      Run Kopuz's `kopuzd` as a systemd user service, so playback, media keys
      and MPRIS keep working with the window closed. Without it the window
      starts kopuzd itself when nothing answers on the socket.

      kopuzd and the Kopuz desktop app own the same library and only one of
      them can run at a time; FormalMusic attaches to whichever is up. Turn
      this off if you use the Kopuz app.
    '';
  };

  config = lib.mkIf cfg.enable {
    systemd.user.services.kopuzd = lib.mkIf (cfg.daemon && pkgs.stdenv.hostPlatform.isLinux) {
      Unit = {
        Description = "Kopuz music daemon";
        # MPRIS and media keys need the session's bus and display.
        After = [
          "pipewire.service"
          "graphical-session.target"
        ];
        PartOf = [ "graphical-session.target" ];
      };
      Service = {
        ExecStart = lib.getExe' kopuz.packages.${pkgs.stdenv.hostPlatform.system}.default "kopuzd";
        Restart = "on-failure";
        RestartSec = 5;
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
  };
}
