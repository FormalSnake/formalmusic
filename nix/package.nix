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
  libxkbfile,
  libgbm,
  openssl,
  xdg-utils,
  ffmpeg-headless,
  kopuz,
  stdenv,
  libicns,
}:

let
  appId = "dev.formalsnake.formalmusic";
  version = (lib.importTOML ../Cargo.toml).workspace.package.version;
  inherit (stdenv.hostPlatform) isDarwin isLinux;
  infoPlist = builtins.toFile "Info.plist" (lib.generators.toPlist { escape = true; } {
    CFBundleDevelopmentRegion = "en";
    CFBundleExecutable = "formalmusic";
    CFBundleIdentifier = appId;
    CFBundleInfoDictionaryVersion = "6.0";
    CFBundleName = "FormalMusic";
    CFBundleDisplayName = "FormalMusic";
    CFBundlePackageType = "APPL";
    CFBundleShortVersionString = version;
    CFBundleVersion = version;
    CFBundleIconFile = "AppIcon";
    LSMinimumSystemVersion = "14.0";
    NSPrincipalClass = "NSApplication";
    NSHighResolutionCapable = true;
  });
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "formalmusic";
  inherit version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../.cargo
      ../crates
      ../packaging
    ];
  };

  # The git dependencies (nativedesktop, kopuz's client crates, libghostty-vt) are pinned by
  # rev in Cargo.lock, which builtins.fetchGit takes without a separate hash.
  cargoLock = {
    lockFile = ../Cargo.lock;
    allowBuiltinFetchGit = true;
  };
  cargoBuildFlags = [
    "--package=formalmusic"
  ]
  # nd's AppKit backend links Swift from Xcode, so the link and rustc's strip use the system
  # tools: Nix's cc wrapper brings its own SDK and Nix's strip corrupts proc-macro dylibs. The
  # hooks set the linker in the environment, which only `--config` outranks. This needs the Nix
  # sandbox off, the default on macOS.
  ++ lib.optional isDarwin ''--config=target.aarch64-apple-darwin.linker="/usr/bin/clang"'';

  nativeBuildInputs = [
    pkg-config
    makeBinaryWrapper
  ] ++ lib.optional isDarwin libicns;

  buildInputs = lib.optionals isLinux [
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

  preBuild = lib.optionalString isDarwin ''
    unset SDKROOT DEVELOPER_DIR
    mkdir -p "$TMPDIR/apple-bin"
    ln -s /usr/bin/strip /usr/bin/xcrun "$TMPDIR/apple-bin/"
    export PATH="$TMPDIR/apple-bin:$PATH"
  '';

  # The window tests need a window server, and the live tests need the network.
  doCheck = false;

  postInstall = lib.optionalString isLinux ''
    install -Dm644 packaging/linux/${appId}.png -t $out/share/icons/hicolor/512x512/apps
    install -Dm644 packaging/linux/${appId}.desktop -t $out/share/applications
  '' + lib.optionalString isDarwin ''
    app=$out/Applications/FormalMusic.app/Contents
    install -Dm644 ${infoPlist} $app/Info.plist
    echo -n 'APPL????' > $app/PkgInfo
    mkdir -p $app/MacOS $app/Resources
    png2icns $app/Resources/AppIcon.icns packaging/icon.png
    mv $out/bin/formalmusic $app/MacOS/formalmusic
    ln -s $app/MacOS/formalmusic $out/bin/formalmusic
  '';

  dontStrip = isDarwin;

  # nd's GPUI backend dlopens the windowing and GPU libraries at runtime. Animated
  # covers and music videos decode with ffmpeg. With no daemon on the socket the
  # window starts kopuzd, the same Kopuz build its client crates come from.
  postFixup = lib.optionalString isDarwin ''
    wrapProgram $out/Applications/FormalMusic.app/Contents/MacOS/formalmusic --suffix PATH : ${
      lib.makeBinPath [
        ffmpeg-headless
        kopuz
      ]
    }
  '' + lib.optionalString isLinux ''
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
    homepage = "https://github.com/FormalStudio/formalmusic";
    license = lib.licenses.mit;
    mainProgram = "formalmusic";
    platforms = lib.platforms.linux ++ [ "aarch64-darwin" ];
  };
})
