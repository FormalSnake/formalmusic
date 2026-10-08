{
  lib,
  stdenv,
  rustPlatform,
  fetchurl,
  pkg-config,
  cmake,
  makeBinaryWrapper,
  alsa-lib,
  libopus,
  openssl,
  ffmpeg-headless,
  xdg-utils,
  kopuz,
}:

let
  # `deno_core` pulls in `v8`, whose build script downloads a prebuilt
  # librusty_v8, which the sandbox cannot. Same pin and hashes as kopuz's own
  # packaging/nix/crane.nix; keep them in step with its Cargo.lock.
  rustyV8Version = "130.0.7";
  rustyV8Target = stdenv.hostPlatform.rust.rustcTarget;
  rustyV8Hashes = {
    "aarch64-unknown-linux-gnu" = "0nli54vqcrfh9nkz7ma7230k0xmhcrk0jmfbyxcp3rxybarygvxy";
    "x86_64-unknown-linux-gnu" = "0pdp6h7vbjvq5l9lh25qilmp6xrxg7mj8m263h44f0lv9swnqix6";
  };
  librustyV8 = fetchurl {
    url = "https://github.com/denoland/rusty_v8/releases/download/v${rustyV8Version}/librusty_v8_release_${rustyV8Target}.a.gz";
    sha256 = rustyV8Hashes.${rustyV8Target};
  };
in
rustPlatform.buildRustPackage {
  pname = "kopuzd";
  version = (lib.importTOML "${kopuz}/Cargo.toml").workspace.package.version;
  src = kopuz;

  cargoLock = {
    lockFile = "${kopuz}/Cargo.lock";
    allowBuiltinFetchGit = true;
  };
  cargoBuildFlags = [ "--package=kopuz-kopuzd" ];

  nativeBuildInputs = [
    pkg-config
    cmake
    makeBinaryWrapper
  ];
  buildInputs = [
    alsa-lib
    libopus
    openssl
  ];

  env = {
    SQLX_OFFLINE = "true";
    RUSTY_V8_ARCHIVE = librustyV8;
    CARGO_PROFILE_RELEASE_DEBUG = "false";
  };

  doCheck = false;

  # kopuzd asks xdg-settings which browser to open for a sign-in, and hands
  # some formats to ffmpeg.
  postFixup = ''
    wrapProgram $out/bin/kopuzd --suffix PATH : ${
      lib.makeBinPath [
        ffmpeg-headless
        xdg-utils
      ]
    }
  '';

  meta = {
    description = "Kopuz's headless daemon, which FormalMusic plays through";
    homepage = "https://github.com/FormalSnake/kopuz";
    license = lib.licenses.eupl12;
    mainProgram = "kopuzd";
    platforms = lib.platforms.linux;
  };
}
