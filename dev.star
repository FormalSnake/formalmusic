# rustup's proxies act on the name they run as and install the pinned toolchain on first use.
RUST = ["cargo", "rustc", "rustdoc", "rustfmt", "cargo-fmt", "cargo-clippy", "clippy-driver"]

# gpui-pre links libxkbcommon and freetype at build time and dlopens wayland, vulkan,
# fontconfig and X11 at runtime; cpal needs alsa.
LINUX = ["xkbcommon", "xkbcommon-x11", "wayland-client", "wayland-protocols", "vulkan", "fontconfig", "freetype2",
         "xcb", "x11", "xcursor", "xi", "xrandr", "egl", "alsa"]

linux = platform.is_linux

dev(
    # The stream worker runs $FORMALMUSIC_YTDLP_PYTHON, an interpreter that imports yt_dlp: the
    # virtualenv pypi("yt-dlp") builds over the pinned CPython.
    packages = [
        pkg("rustup", links = {name: "rustup" for name in RUST}),
        pkg("python", version = "3.13"),
        pypi("yt-dlp"),
    ],
    # symphonia-adapter-libopus links the system libopus through pkg-config on every platform.
    libs = [lib(n) for n in ["opus"] + (LINUX if linux else [])],
    bins = [bin(n) for n in ["pkg-config", "socat", "ffmpeg"] + (["gcc", "grim"] if linux else [])],
    env = {"RUSTUP_TOOLCHAIN": "1.99.1"},
    shell_hook = 'export FORMALMUSIC_YTDLP_PYTHON="$FSPEC_PKG_YT_DLP/bin/python"',
)
