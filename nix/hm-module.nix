{ self, nativedesktop }:
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
  # read-only in the app's Settings window, and the daemon plays by the same file.
  imports = [
    (import "${nativedesktop}/nix/hm-module.nix" {
      pname = "formalmusic";
      settingsOptions = import ./settings-options.nix;
      defaultPackage = system: self.packages.${system}.default;
    })
  ];

  options.programs.formalmusic = {
    daemon = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Run `formalmusicd` as a systemd user service, so playback, media keys
        and MPRIS keep working with the window closed. Without it the app
        starts the daemon itself and it exits with the session.
      '';
    };

    lastfm = {
      apiKeyFile = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        example = "/run/agenix/lastfm-api-key";
        description = ''
          File holding the API key of your Last.fm API account
          (last.fm/api/account/create), read by `formalmusicd` at start. A
          path string rather than a Nix path, so the secret stays out of the
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

  config = lib.mkIf cfg.enable {
    systemd.user.services.formalmusicd = lib.mkIf (cfg.daemon && pkgs.stdenv.hostPlatform.isLinux) {
      Unit = {
        Description = "FormalMusic playback daemon";
        After = [ "pipewire.service" ];
      };
      Service = {
        ExecStart = lib.getExe' cfg.package "formalmusicd";
        Restart = "on-failure";
        Environment =
          lib.optional (
            cfg.lastfm.apiKeyFile != null
          ) "FORMALMUSIC_LASTFM_API_KEY_FILE=${cfg.lastfm.apiKeyFile}"
          ++ lib.optional (
            cfg.lastfm.sharedSecretFile != null
          ) "FORMALMUSIC_LASTFM_SECRET_FILE=${cfg.lastfm.sharedSecretFile}";
      };
      Install.WantedBy = [ "default.target" ];
    };
  };
}
