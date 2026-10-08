{
  lib,
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
  xdg-utils,
  ffmpeg-headless,
  kopuzd,
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
      ../patches
    ];
  };

  # kopuz-client and kopuz-api come from git at the rev Cargo.toml pins.
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

  # GPUI dlopens the windowing and GPU libraries at runtime. The app decodes
  # animated covers with ffmpeg and ffprobe; the headless build has the native
  # H.264 decoder they need. It starts kopuzd itself when no service runs it,
  # so the daemon this package was built against goes on its PATH.
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
    wrapProgram $out/bin/formalmusic --prefix PATH : ${lib.makeBinPath [ kopuzd ]} \
      --suffix PATH : ${
        lib.makeBinPath [
          xdg-utils
          ffmpeg-headless
        ]
      }
  '';

  passthru = { inherit kopuzd; };

  meta = {
    description = "YouTube Music client for Linux";
    homepage = "https://github.com/FormalSnake/formalmusic";
    license = lib.licenses.mit;
    mainProgram = "formalmusic";
    platforms = lib.platforms.linux;
  };
})
