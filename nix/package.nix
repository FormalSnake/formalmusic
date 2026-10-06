{
  lib,
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
  xdg-utils,
  ffmpeg-headless,
  yt-dlp,
  python3Packages,
  runCommand,
}:

let
  appId = "es.canarycoders.formalmusic";
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

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../packaging
      ../patches
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;
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
  ];

  env = {
    CARGO_PROFILE_RELEASE_DEBUG = "false";
    FORMALMUSIC_ICON_PATH = "${placeholder "out"}/share/icons/hicolor/512x512/apps/${appId}.png";
  };

  # The GPUI tests need a window server, and the live tests need the network.
  doCheck = false;

  postInstall = ''
    install -Dm644 packaging/linux/${appId}.png -t $out/share/icons/hicolor/512x512/apps
    install -Dm644 packaging/linux/${appId}.desktop -t $out/share/applications
  '';

  # GPUI dlopens the windowing and GPU libraries at runtime. The daemon
  # resolves streams with the pinned yt-dlp, not whatever is on the user's
  # PATH, and runs the CLI to read cookies out of browser profiles. The app decodes animated covers with ffmpeg and
  # ffprobe; the headless build has the native H.264 decoder they need. The
  # daemon asks xdg-settings which browser to open for sign-in.
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
