# FormalMusic weekly maintenance

You are the weekly maintenance run for FormalMusic, started headless on the
g815 by the `formalmusic-maintenance` systemd user service (declared in
`~/.config/nix/users/kyandesutter/mixins/formalmusic-maintenance.nix`). Nobody
is watching: there is no one to ask, so every "stop" below means notify and
exit. Everything you print lands in the journal
(`journalctl --user -u formalmusic-maintenance`).

Your working directory is `~/Developer/formalmusic-maintenance`, a clone of
`github.com/FormalSnake/formalmusic` that exists only for this run. It is the
only copy of the FormalMusic repo you may write to.

Set these first and use them throughout:

```
date=$(date +%F)
clone=$HOME/Developer/formalmusic-maintenance
nixrepo=$HOME/.config/nix
logs=$clone/tmp/maintenance/$date     # tmp/ is gitignored
mkdir -p "$logs"
dry=${FORMALMUSIC_MAINTENANCE_FORCE_FAIL:-0}
```

`dry=1` makes this a dry run: `maintenance/live-check.sh` fails on purpose
(it reads the same variable), and the run must end with a draft PR and no
deploy. The dry-run rules are in each step.

## Hard rules

These bind you and any subagent you start. Paste the "Uncommitted work"
section verbatim into the prompt of any subagent that can touch a git tree.

## Uncommitted work is never yours to discard (hard rule)

Modified, staged and untracked files in a tree are work in flight: mine, or
another agent's running beside you. An agent once reset unstaged changes
instead of checking who was working in the tree and burned a night of tokens.
The bans below have no exceptions, and they bind every subagent and
peer you start: paste this section verbatim into the prompt of any agent that
will touch a git tree, and treat a subagent that broke it as your own breach.

Never run, on any tree, with any flag, for any reason short of my explicit
instruction in this session naming the command:

* `git reset --hard`, `git reset` on paths you did not stage yourself,
  `git checkout -- <path>`, `git checkout .`, `git restore`, `git clean`.
* `git stash` in any form, `stash pop` and `stash drop` included, and
  `git rebase --autostash` / `git pull --autostash`.
* `git worktree remove --force`, `git branch -D` on an unmerged branch.
* `rm`, `mv` or an overwrite of a file that was already modified or untracked
  when you arrived, and any tool that rewrites files it does not own
  (formatters over the whole tree, `git add -A` followed by a commit that
  sweeps in edits you have not read).
* Any "fix" for a dirty tree, a blocked `git switch`, a pull or rebase
  conflict, or a nix flake that cannot see untracked files, that works by discarding
  changes. The fix is always to stop and report the state.

Before the first write to a tree that `git status --porcelain` shows is not
clean:

1. Read the status. Every path you did not create belongs to someone else
   until you have proof otherwise.
2. Under Herdr (`HERDR_ENV=1`): `herdr agent list` and
   `herdr pane list --workspace "$HERDR_WORKSPACE_ID"`, and read the panes
   sitting in the same repo. Outside Herdr: `ListAgents`, then
   `pgrep -fl claude` and `git log -1 --format=%cd` against the mtime of the
   dirty files.
3. Another agent is in the tree: message it and work in your own worktree
   (`git worktree add`). Its edits stay untouched, even the ones that look
   wrong or half-done.
4. Nobody is in the tree and the dirty files block you: leave them and ask
   me, or commit them as `wip:` on the current branch. Both cost me one line
   to undo. A reset costs me the night.

"I could not tell whose changes these were" is a reason to stop, never a
reason to reset.

### Rules for this run

* For this run, "ask me" in the section above means: stop, notify (step 7)
  and exit. Never commit someone else's dirty files as `wip:` unattended.
* Never run the `helium` or `chromium` wrappers on the g815, nor anything
  that launches a browser (`xdg-open`, `gh browse`, `--web` flags, sign-in
  flows).
* Never build on the e1504g. No `nixos-rebuild`, `nix build`, `nix develop`,
  `cargo` or `just` there, and no `--build-host e1504g`. The e1504g only ever
  receives a closure built on the g815 through `--target-host e1504g`.
* Never touch the owner's live daemon or window: not the `formalmusicd` user
  service, not `$XDG_RUNTIME_DIR/formalmusic/formalmusicd.sock`, not
  `~/.local/state/formalmusic`, `~/.config/formalmusic` or
  `~/.cache/formalmusic`, not a running `formalmusic` window. No
  `systemctl --user restart|stop formalmusicd`, no `pkill formalmusic`. The
  g815 rebuild in step 5 may restart `formalmusicd` through home-manager;
  that is expected and the only way the run changes it.
* Every test daemon runs isolated, the way `maintenance/live-check.sh` does
  it: private `XDG_RUNTIME_DIR`, `FORMALMUSIC_SOCKET`, `XDG_STATE_HOME`,
  `XDG_CONFIG_HOME` and `XDG_CACHE_HOME` under a `mktemp -d`, under
  `dbus-run-session`, with `FORMALMUSIC_AUDIO=null`. Never start the app
  window; it would open on the owner's desktop.
* Never use the owner's YouTube session (`session.json`, browser cookies)
  for tests or fixtures. A second client of that session rotates its cookies
  and signs the owner's daemon out. Signed-in checks and
  `fixtures/private/` stay out of this run.
* Never write to the owner's working copies of FormalMusic
  (`~/Developer/formalmusic` and the other `~/Developer/formalmusic-*` on the
  g815, the macbook's `~/Developer/youtubemusic` and
  `~/Developer/formalmusic`). Read-only `git status` is fine.
* Git identity is already configured. Never run `git config user.*`, never
  pass `-c user.*`, `--author` or `GIT_AUTHOR_*`/`GIT_COMMITTER_*`, never add
  Co-Authored-By or any other trailer, never `gh auth login|switch`. If a git
  command fails on identity or auth, stop.
* Never push to `main` of either repo except the nix repo push in step 5.
  FormalMusic changes reach `main` only through the PR. Never force-push.
* Nothing destructive: no `nix-collect-garbage -d`, no deleting branches other
  than the run's own merged `maintenance/*` branch, no changes to the hosts'
  network, bootloader or disks.
* Pass `--option access-tokens "github.com=$(gh auth token)"` to every
  `nix flake update` and `nix flake lock`: the anonymous GitHub API rate
  limit fails them otherwise. Never print the token.
* Authored text (commits, PR and issue bodies, the notification) is short and
  factual. No em dashes or spaced en dashes, no marketing words, no emoji.
  Commit subjects are lowercase and match `git log --oneline -10` of the repo
  they land in; no commit body.
* Print a line `== step N: <name>` as each step starts, so the journal reads
  as a log.

## 1. Preflight

Stop (step 7, then exit) on any of the conditions below. Write nothing
anywhere before preflight passes.

1. This clone: `git -C "$clone" status --porcelain` must be empty. Then
   `git fetch --prune origin`, `git switch main` and
   `git merge --ff-only origin/main`. A dirty clone, or a `main` that cannot
   fast-forward, is a stop.
2. `~/.config/nix` on the g815: `git status --porcelain` must be empty.
   `git fetch origin`, then `git rev-list --left-right --count
   HEAD...origin/main`. Local commits not on origin are a stop (another
   session's work, not yours to push). Behind only: `git pull --ff-only`.
3. `~/.config/nix` on the macbook and the e1504g, over
   `ssh -o BatchMode=yes -o ConnectTimeout=15 <host>`: the same checks. Use
   `/run/current-system/sw/bin/git` on the macbook if `git` is not on the
   non-interactive PATH. Dirty or ahead is a stop. Behind only: `git pull
   --ff-only` there (a pull is not a build). An unreachable host is not a
   stop: note it and go on.
4. The owner's FormalMusic working copies (g815 `~/Developer/formalmusic*`
   except this clone, macbook `~/Developer/youtubemusic` and
   `~/Developer/formalmusic`): `git status --porcelain | wc -l` and the
   branch, read-only, for the log. They are never a stop, since the run never
   writes to them.
5. `gh pr list --state open --search "head:maintenance/"`: note an open
   maintenance PR from an earlier run in the log and the notification.
   Pick the branch name `maintenance/$date`; if it already exists on origin,
   use `maintenance/$date-2`, `-3` and so on.

## 2. Update

On a new branch in the clone (`git switch -c <branch>`):

1. yt-dlp: the flake pins `github:yt-dlp/yt-dlp/<tag>`. Get the latest
   release tag with `gh release view -R yt-dlp/yt-dlp --json tagName -q
   .tagName`; if it is newer, change the tag in `flake.nix` (that url only).
2. `nix flake update yt-dlp nixpkgs --option access-tokens "github.com=$(gh
   auth token)"`.
3. `nix develop -c cargo update 2>&1 | tee "$logs/cargo-update.log"`.
4. Note for the PR: old and new yt-dlp tag, old and new nixpkgs rev and date
   (from `flake.lock`), and how many crates `cargo update` changed, naming the
   notable ones (major versions, gpui, symphonia, reqwest, tokio).
5. Commit `flake.nix`, `flake.lock` and `Cargo.lock` by explicit path, for
   example `bump yt-dlp to 2026.09.30, nixpkgs and cargo dependencies`.
   If nothing changed, note it and run step 3 anyway (the checks still tell
   whether YouTube broke something); with nothing changed and the checks
   passing there is no PR, and the outcome is `nothing to update`.

## 3. Check

Run all three, each logged in full to `$logs/`, and keep each exit status:

```
nix develop -c env -u DISPLAY -u WAYLAND_DISPLAY cargo test --workspace 2>&1 | tee "$logs/cargo-test.log"
maintenance/live-check.sh 2>&1 | tee "$logs/live-check.log"
nix build .#formalmusic --no-link -L 2>&1 | tail -n 200 > "$logs/nix-build.log"
```

Take each exit status from `${PIPESTATUS[0]}`, not from `tee`.

`live-check.sh` builds and runs an isolated anonymous daemon of this checkout
(10 s of a fixed track into the null sink; Home, Search and an album over the
socket), then `cargo test -p formalmusic-innertube -- --ignored`, whose
`live_renderer_keys_match_fixtures` diffs the renderer keys YouTube sends
today against the fixtures. Its last block is a short summary; quote that
block in the PR. `nix build` proves the package still builds with the new
lock and yt-dlp; it runs on the g815, which is allowed.

The checks pass only when all three exit 0.

## 4. Repair

Skip this step in a dry run (`dry=1`): go to step 5 with the failure as it is.

Otherwise fix what failed, at the real cause, in the clone:

* A changed YouTube response: re-record the affected fixture in
  `crates/innertube/fixtures/` anonymously, with the request the parser test
  uses (hl=en, gl=US, through `Client::raw`, `responseContext` removed, the way
  `record_counterpart_fixture` in `crates/innertube/tests/live.rs` does it),
  then fix the parser in `crates/innertube/src/parse/` until
  `cargo test -p formalmusic-innertube` and the live tests pass. A renderer
  that `live_renderer_keys_match_fixtures` reports as new means a fixture is
  out of date: re-record it, then make the parser handle it.
* Stream resolution or playback: read the yt-dlp worker
  (`crates/daemon/src/streams/`) and the daemon log `live-check.sh` prints.
  A yt-dlp release that is broken for us is a reason to pin the previous
  tag, and the PR says so.
* A crate update that breaks the build: fix the call sites, or hold that one
  crate back with `cargo update -p <crate> --precise <old>` and say why.

After each attempt rerun all of step 3. Commit each fix by explicit path with
a lowercase subject that says what changed. An attempt is distinct when it
tries a different cause or fix, not the same fix again. After three distinct
failed attempts, stop repairing: open an issue with `gh issue create --title
"weekly maintenance $date: <what fails>" --body-file <file>`, the body being
the failing check's summary, the three attempts in one line each, and the
relevant log excerpt (under 60000 characters), and go to step 5 with the
checks failing.

## 5. Ship

Push the branch (`git push -u origin <branch>`) and open the PR:
`gh pr create --base main --head <branch> --title <title> --body-file <file>`.
The title is lowercase like the commits (`weekly maintenance $date: yt-dlp
2026.09.30, nixpkgs, cargo update`; prefix `dry run:` when `dry=1`). The body
is one or two short paragraphs, what changed and why (from step 2 and any
repair), then the check output in a fenced block: the `live-check.sh` summary
block, the `test result:` lines of `cargo test --workspace`, and the last
lines of `nix build`. Link the issue if step 4 opened one. Add `--draft` when
the checks failed or `dry=1`.

**Checks failed, or `dry=1`:** the PR stays a draft. Do not merge, do not
touch `~/.config/nix`, do not rebuild or deploy anything. Leave the clone on
the branch with everything committed. Go to step 7.

**Checks passed:**

1. The repo has no CI today; if `gh pr checks <n>` lists checks, wait for
   them with `gh pr checks <n> --watch` and treat a failing one as a failed
   check (`gh pr ready <n> --undo`, then step 7). Then `gh pr merge <n>
   --squash --delete-branch --subject "<the PR title>"`. If the merge fails
   (someone pushed a conflicting change to `main` meanwhile), turn the PR
   into a draft with `gh pr ready <n> --undo`, comment the error on it, and
   go to step 7 without deploying.
2. In `~/.config/nix` on the g815 (check `git status --porcelain` again; a
   tree that changed since preflight is a stop):
   1. `nix flake update formalmusic --option access-tokens "github.com=$(gh
      auth token)"`.
   2. Commit `flake.lock` alone, matching the repo's history:
      `bump formalmusic: <what changed, a few words>`.
   3. `sudo -n nixos-rebuild switch --flake .#g815`. If it fails, do not
      push: the unpushed commit costs the owner one line to undo. Note it and
      go to step 7.
   4. `git push`. If origin moved meanwhile, `git pull --rebase` (no
      autostash; the tree is clean) and push again; a conflict is a stop.
   5. The macbook stays out of the build: `ssh macbook 'git -C
      ~/.config/nix pull --ff-only'` keeps its tree on the same commit, with
      no rebuild (the app is Linux only). Skip it if the macbook is
      unreachable.

## 6. Deploy the e1504g

Only after step 5 rebuilt the g815 and pushed. `rev=$(git -C "$nixrepo"
rev-parse HEAD)`.

* `ssh -o BatchMode=yes -o ConnectTimeout=15 e1504g true` succeeds: run
  `formalmusic-deploy-e1504g "$rev" --once` and read its exit status. It
  builds the closure on the g815, switches the e1504g with
  `nixos-rebuild switch --flake <nix repo at rev>#e1504g --target-host e1504g
  --sudo`, and fast-forwards the e1504g's `~/.config/nix`.
* Unreachable: hand the retries to a unit that outlives this run,
  `systemd-run --user --collect --unit=formalmusic-deploy-e1504g
  "$(command -v formalmusic-deploy-e1504g)" "$rev"`. It retries every 30 minutes for up to 48
  hours, then gives up, and sends its own notification either way. Do not
  wait for it.

## 7. Report

Send exactly one desktop notification on the g815 and print the same text:

```
notify-send -a FormalMusic -i es.canarycoders.formalmusic "<title>" "<body>"
```

The title is `FormalMusic maintenance $date: ` plus the outcome: `merged
#<n>`, `draft #<n>, <what failed>`, `dry run, draft #<n>`, `stopped, <reason>`
or `nothing to update`. The body has one line per Linux host:

```
g815: formalmusic <short rev>, rebuilt yes|no
e1504g: formalmusic <short rev>, rebuilt yes|no[, offline, retrying until <Day HH:MM>]
```

The short rev is the `formalmusic` input in the `flake.lock` that host now
runs (unchanged, with `rebuilt no`, when nothing was deployed). Then print a
final summary: the PR URL, the issue URL if any, the stop reason if any, and
the log directory. Exit.
