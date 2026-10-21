# nd port: look and feel against the old FormalMusic

The user's report on the nd port (Linux, GPUI) and on macOS, item by item, with the old GPUI
window (`git show 88728dd^:crates/desktop/src`) as the reference for layout and controls. A
tick means fixed and seen on screen on both backends; nd fixes live on nativedesktop branch
`formalmusic-ui`.

- [x] 1. Sidebar playlists all showed the same document icon. Playlists show the playlist
      symbol, the navigation rows Home, Explore, Library and History their own.
- [x] 2. Spacing: content touched the sidebar and the header. Pages sit on the old 32 pt
      gutter, sections 28 pt apart, list rows inset so art lines up with the headings.
- [x] 3. Player bar: text buttons for like, dislike, repeat and shuffle. Rebuilt on nd's
      `player_bar`: transport at the leading end, the song with like, dislike and more
      actions in the middle, the seek bar with elapsed and total time, volume, repeat,
      shuffle, queue and expand as icon toggles that show their state.
- [x] 4. A stray "More" button under playlist descriptions. A long description is clamped
      to three lines and opens in place when clicked; a short one shows whole.
- [x] 5. Song rows had no hover state. Track and item rows take a rounded hover wash; nd's
      inset and table lists draw one for every row.
- [x] 6. The player could not be expanded. Clicking the playing song or the expand button
      opens the expanded player, the queue button opens it on Up next.
- [ ] 7. Sign-in did not work again. Linux had no way back to Sign In once the prompt was
      dismissed (no menu bar on GPUI): the header bar has a main menu and the sidebar a
      Sign in row. Pasted cookies on an anonymous Kopuz source left it anonymous, so the
      session read signed out: the source is switched to signed-in before provisioning.
      The menu and row are seen on screen; the Kopuz path is checked by reading only, a
      real sign-in is still open.
- [x] 8. GTK look on Linux: nd widgets and tokens only, libadwaita metrics from nd.
- [x] 9. macOS trackpad scrolling stuttered, vertically and on the Home shelves. A
      vertical gesture over a shelf goes to the page whole; shelves snap after the
      gesture and its momentum end, not during a pause.
- [x] 10. macOS look closer to Music: source list sidebar with tinted symbols, the toolbar's
      back, forward and search capsules, the player bar along the window's foot.

Screenshots from `cargo test -p formalmusic --test look` with `ND_REAL=1 ND_SHOT=<dir>`.
