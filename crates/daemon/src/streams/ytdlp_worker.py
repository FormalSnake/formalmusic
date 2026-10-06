"""yt-dlp kept loaded for formalmusicd.

One JSON request per line on stdin, one JSON answer per line on stdout:

    {"id": 1, "video_id": "...", "cookies": "/path" or null, "premium": false}
    {"id": 1, "info": {"formats": [...]}, "cookies": {...}}  or  {"id": 1, "error": "..."}

`cookies` in an answer are the youtube.com cookies yt-dlp holds after the
request, rotated ones included, for the daemon to keep.

Requests run on a few threads, so a track resolved ahead never holds up the
one the user asked for. The interpreter, the extractors and the player JS stay
loaded between tracks; a fresh `yt-dlp -J` spends about a second on those.
"""

import json
import os
import signal
import sys
import threading
from concurrent.futures import ThreadPoolExecutor

import yt_dlp

# Off in yt-dlp by default. On, the player JS reduced to its challenge
# functions is kept in the cache dir, and deno solves a track's challenges
# from it in about 0.25 s instead of 0.6 s.
try:
    from yt_dlp.extractor.youtube.jsc._builtin.ejs import EJSBaseJCP

    EJSBaseJCP._ENABLE_PREPROCESSED_PLAYER_CACHE = True
except ImportError:
    pass

# A Premium session gets its streams from the web_music client alone. Left to
# itself, yt-dlp fetches the watch page, the config of each client it tries and
# a `next` response just to learn the account has Premium: three seconds a track.
PREMIUM_ARGS = {
    "player_client": ["web_music"],
    "player_skip": ["webpage", "configs", "initial_data"],
    # Premium streams need no GVS PO token, but without the `next` response
    # yt-dlp cannot tell the account has Premium and would drop them.
    "formats": ["missing_pot"],
}


class Log:
    def debug(self, msg):
        pass

    def info(self, msg):
        pass

    def warning(self, msg):
        print(msg, file=sys.stderr, flush=True)

    def error(self, msg):
        print(msg, file=sys.stderr, flush=True)


class Pool:
    """Idle YoutubeDL instances per (signed in, premium). An instance is used
    by one thread at a time and reloads its jar when the daemon hands it a
    newer cookie file."""

    def __init__(self, cache_dir):
        self.cache_dir = cache_dir
        self.idle = {}
        self.lock = threading.Lock()

    def take(self, cookies, premium):
        key = (bool(cookies), premium)
        with self.lock:
            ydl = self.idle[key].pop() if self.idle.get(key) else None
        if ydl is None:
            ydl = self.new(cookies, premium)
        if cookies and os.path.exists(cookies):
            loaded = (cookies, os.stat(cookies).st_mtime_ns)
            if getattr(ydl, "formalmusic_cookies", None) != loaded:
                ydl.cookiejar.clear()
                ydl.cookiejar.load(cookies)
                ydl.formalmusic_cookies = loaded
        return key, ydl

    def new(self, cookies, premium):
        opts = {
            "logger": Log(),
            "quiet": True,
            "noplaylist": True,
            "skip_download": True,
            "cachedir": self.cache_dir,
        }
        # The daemon's private copy; yt-dlp only writes the jar back on close(),
        # which is never called here.
        if cookies:
            opts["cookiefile"] = cookies
        if cookies and premium:
            opts["extractor_args"] = {"youtube": PREMIUM_ARGS}
        return yt_dlp.YoutubeDL(opts)

    def give(self, key, ydl):
        with self.lock:
            self.idle.setdefault(key, []).append(ydl)


def youtube_cookies(ydl):
    return {
        c.name: c.value
        for c in ydl.cookiejar
        if c.domain.lstrip(".").endswith("youtube.com") and c.value is not None
    }


def formats(info):
    out = []
    for f in info.get("formats") or []:
        f = dict(f)
        f.pop("fragments", None)
        out.append(f)
    return out


def main():
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    pool = Pool(sys.argv[1] if len(sys.argv) > 1 else None)
    out = threading.Lock()

    def answer(message):
        line = json.dumps(message, separators=(",", ":"))
        with out:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()

    def handle(request):
        rid = request.get("id")
        try:
            key, ydl = pool.take(request.get("cookies"), bool(request.get("premium")))
            try:
                url = "https://music.youtube.com/watch?v=" + request["video_id"]
                info = ydl.sanitize_info(ydl.extract_info(url, download=False))
                jar = youtube_cookies(ydl) if request.get("cookies") else {}
            finally:
                pool.give(key, ydl)
            answer({"id": rid, "info": {"formats": formats(info)}, "cookies": jar})
        except Exception as e:
            answer({"id": rid, "error": str(e) or type(e).__name__})

    # Loads the YouTube extractor before the first request needs it.
    key, ydl = pool.take(None, False)
    ydl.get_info_extractor("Youtube")
    pool.give(key, ydl)
    with ThreadPoolExecutor(max_workers=4) as executor:
        for line in sys.stdin:
            if line.strip():
                executor.submit(handle, json.loads(line))


if __name__ == "__main__":
    main()
