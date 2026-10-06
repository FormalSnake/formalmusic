self:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.programs.formalmusic;
  json = pkgs.formats.json { };
in
{
  options.programs.formalmusic = {
    enable = lib.mkEnableOption "FormalMusic, a YouTube Music client";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "formalmusic.packages.\${system}.default";
      description = "The FormalMusic package to install.";
    };

    daemon = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Run `formalmusicd` as a systemd user service, so playback, media keys
        and MPRIS keep working with the window closed. Without it the app
        starts the daemon itself and it exits with the session.
      '';
    };

    theme = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      example = {
        canvas = "#1c1917";
        text = "#b4bdc3";
        accent = "#6099c0";
      };
      description = ''
        Palette tokens written to `~/.config/formalmusic/theme.json`. The names
        are the fields of `Palette` in `crates/desktop/src/theme.rs`. Leave
        empty to let another tool (matugen, for one) own the file.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];

    xdg.configFile."formalmusic/theme.json" = lib.mkIf (cfg.theme != { }) {
      source = json.generate "formalmusic-theme.json" cfg.theme;
    };

    systemd.user.services.formalmusicd = lib.mkIf cfg.daemon {
      Unit = {
        Description = "FormalMusic playback daemon";
        After = [ "pipewire.service" ];
      };
      Service = {
        ExecStart = lib.getExe' cfg.package "formalmusicd";
        Restart = "on-failure";
      };
      Install.WantedBy = [ "default.target" ];
    };
  };
}
