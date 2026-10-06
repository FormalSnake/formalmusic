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
}:

let
  appId = "es.canarycoders.formalmusic";
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

  # GPUI dlopens the windowing and GPU libraries at runtime. The daemon shells
  # out to yt-dlp for stream URLs, so it gets the pinned one, not whatever is
  # on the user's PATH. The app decodes animated covers with ffmpeg and
  # ffprobe; the headless build has the native H.264 decoder they need.
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
    wrapProgram $out/bin/formalmusicd --prefix PATH : ${lib.makeBinPath [ yt-dlp ]}
  '';

  meta = {
    description = "YouTube Music client for Linux";
    homepage = "https://github.com/FormalSnake/formalmusic";
    license = lib.licenses.mit;
    mainProgram = "formalmusic";
    platforms = lib.platforms.linux;
  };
})
