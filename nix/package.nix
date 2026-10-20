{
  lib,
  nativedesktop,
  rustPlatform,
  pkg-config,
  makeBinaryWrapper,
  fontconfig,
  freetype,
  libxkbcommon,
  wayland,
  vulkan-loader,
  libGL,
  libx11,
  libxcb,
  libxcursor,
  libxi,
  libxrandr,
  libxkbfile,
  libgbm,
  openssl,
  xdg-utils,
  ffmpeg-headless,
  kopuz,
  runCommand,
}:

let
  appId = "dev.formalsnake.formalmusic";
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "formalmusic";
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;

  # The app depends on nativedesktop by path (`../nativedesktop`), so the
  # build tree holds both checkouts side by side.
  src = runCommand "formalmusic-src" { } ''
    mkdir -p $out
    cp -r ${
      lib.fileset.toSource {
        root = ../.;
        fileset = lib.fileset.unions [
          ../Cargo.toml
          ../Cargo.lock
          ../.cargo
          ../crates
          ../packaging
        ];
      }
    } $out/youtubemusic
    cp -r ${nativedesktop} $out/nativedesktop
    chmod -R u+w $out
  '';
  sourceRoot = "formalmusic-src/youtubemusic";

  # The git dependencies (kopuz's client crates, libghostty-vt) are pinned by rev in
  # Cargo.lock, which builtins.fetchGit takes without a separate hash.
  cargoLock = {
    lockFile = ../Cargo.lock;
    allowBuiltinFetchGit = true;
  };
  cargoBuildFlags = [ "--package=formalmusic" ];

  nativeBuildInputs = [
    pkg-config
    makeBinaryWrapper
  ];

  buildInputs = [
    fontconfig
    freetype
    libxkbcommon
    wayland
    libxcb
    libxkbfile
    openssl
  ];

  env = {
    CARGO_PROFILE_RELEASE_DEBUG = "false";
  };

  # The window tests need a window server, and the live tests need the network.
  doCheck = false;

  postInstall = ''
    install -Dm644 packaging/linux/${appId}.png -t $out/share/icons/hicolor/512x512/apps
    install -Dm644 packaging/linux/${appId}.desktop -t $out/share/applications
  '';

  # nd's GPUI backend dlopens the windowing and GPU libraries at runtime. Animated
  # covers and music videos decode with ffmpeg. With no daemon on the socket the
  # window starts kopuzd, the same Kopuz build its client crates come from.
  postFixup = ''
    patchelf $out/bin/formalmusic --add-rpath ${
      lib.makeLibraryPath [
        vulkan-loader
        wayland
        libxkbcommon
        libGL
        libx11
        libxcb
        libxcursor
        libxi
        libxrandr
        libxkbfile
        libgbm
        fontconfig
        freetype
      ]
    }
    wrapProgram $out/bin/formalmusic --suffix PATH : ${
      lib.makeBinPath [
        xdg-utils
        ffmpeg-headless
        kopuz
      ]
    }
  '';

  passthru = { inherit kopuz; };

  meta = {
    description = "YouTube Music client for Linux";
    homepage = "https://github.com/FormalSnake/formalmusic";
    license = lib.licenses.mit;
    mainProgram = "formalmusic";
    platforms = lib.platforms.linux;
  };
})
