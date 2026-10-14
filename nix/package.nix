{
  lib,
  nativedesktop,
  rustPlatform,
  pkg-config,
  makeBinaryWrapper,
  alsa-lib,
  libopus,
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
  yt-dlp,
  python3Packages,
  runCommand,
}:

let
  appId = "dev.formalsnake.formalmusic";
  # The daemon keeps yt-dlp loaded in one Python process instead of running
  # the CLI per track. This interpreter imports the pinned yt-dlp, whose
  # deno path is already patched into its source.
  ytdlpPython =
    runCommand "formalmusic-ytdlp-python" { nativeBuildInputs = [ makeBinaryWrapper ]; }
      ''
        makeWrapper ${python3Packages.python.withPackages (_: yt-dlp.dependencies)}/bin/python3 \
          $out/bin/formalmusic-ytdlp-python \
          --prefix PYTHONPATH : ${yt-dlp}/${python3Packages.python.sitePackages}
      '';
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

  cargoLock = {
    lockFile = ../Cargo.lock;
    outputHashes."libghostty-vt-0.2.1" = lib.fakeHash;
  };
  cargoBuildFlags = [
    "--package=formalmusic"
    "--package=formalmusicd"
  ];

  nativeBuildInputs = [
    pkg-config
    makeBinaryWrapper
  ];

  buildInputs = [
    alsa-lib
    libopus
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

  # nd's GPUI backend dlopens the windowing and GPU libraries at runtime. The daemon
  # resolves streams with the pinned yt-dlp, not whatever is on the user's
  # PATH, and runs the CLI to read cookies out of browser profiles. The demo
  # paints its animated cover with ffmpeg. The daemon asks xdg-settings which
  # browser to open for sign-in.
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
      ]
    }
    wrapProgram $out/bin/formalmusicd --prefix PATH : ${lib.makeBinPath [ yt-dlp ]} \
      --suffix PATH : ${lib.makeBinPath [ xdg-utils ]} \
      --set FORMALMUSIC_YTDLP_PYTHON ${lib.getExe' ytdlpPython "formalmusic-ytdlp-python"}
  '';

  passthru = { inherit ytdlpPython; };

  meta = {
    description = "YouTube Music client for Linux";
    homepage = "https://github.com/FormalSnake/formalmusic";
    license = lib.licenses.mit;
    mainProgram = "formalmusic";
    platforms = lib.platforms.linux;
  };
})
