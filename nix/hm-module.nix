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
        Run `kopuzd` as a systemd user service, so playback, media keys and
        MPRIS keep working with the window closed. Without it the app starts
        the daemon itself and it exits with the session.
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

    lastfm = {
      apiKeyFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/agenix/lastfm-api-key";
        description = ''
          File holding the API key of your Last.fm API account
          (last.fm/api/account/create), read by `kopuzd` at start. A path
          string rather than a Nix path, so the secret stays out of the
          store. Set it with `sharedSecretFile`; without both, Settings asks
          for the pair.
        '';
      };

      sharedSecretFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/agenix/lastfm-shared-secret";
        description = "File holding the shared secret of the same Last.fm API account.";
      };
    };
  };

  config = lib.mkIf cfg.enable (
    let
      kopuzd = cfg.package.kopuzd;
      # kopuzd takes any config key from KOPUZ_CONFIG_<KEY>, read once at
      # start, so the secrets are read from their files only then.
      start = pkgs.writeShellScript "kopuzd-start" (
        lib.optionalString (cfg.lastfm.apiKeyFile != null) ''
          KOPUZ_CONFIG_LASTFM_API_KEY=$(cat ${lib.escapeShellArg cfg.lastfm.apiKeyFile})
          export KOPUZ_CONFIG_LASTFM_API_KEY
        ''
        + lib.optionalString (cfg.lastfm.sharedSecretFile != null) ''
          KOPUZ_CONFIG_LASTFM_API_SECRET=$(cat ${lib.escapeShellArg cfg.lastfm.sharedSecretFile})
          export KOPUZ_CONFIG_LASTFM_API_SECRET
        ''
        + ''
          exec ${lib.getExe kopuzd}
        ''
      );
    in
    {
    home.packages = [ cfg.package ];

    xdg.configFile."formalmusic/theme.json" = lib.mkIf (cfg.theme != { }) {
      source = json.generate "formalmusic-theme.json" cfg.theme;
    };

    systemd.user.services.kopuzd = lib.mkIf cfg.daemon {
      Unit = {
        Description = "Kopuz playback daemon, for FormalMusic";
        After = [ "pipewire.service" ];
      };
      Service = {
        ExecStart = start;
        Restart = "on-failure";
      };
      Install.WantedBy = [ "default.target" ];
    };
  });
}
