# Android plan

Where the Android client stands against the desktop app, and what to change to
make it feel like a modern streaming app rather than a settings form with a
player attached. Written 2026-10-03 from a source review and four screenshots
(Library, Title, Shares, Settings) taken on a phone running v0.5.0.

Android last caught up with the desktop on 2026-08-10 (`4ef6225`); the desktop
gained about twenty features on 2026-10-02 that Android does not have.

## 0. CI (done, unverified)

Every `Android` workflow run since August failed in `:app:buildRustHost`:
`pstr-core` pulls `keyring` → `dbus-secret-service` → `libdbus-sys`, which needs
`libdbus-1-dev` on the host. Because no run ever succeeded, no cache was ever
saved, so each run started from nothing.

- `libdbus-1-dev` added to both jobs' apt install.
- `Swatinem/rust-cache` shares one key between the two jobs and saves on failure.
- `android/app/build/generated/mpv` is cached on the hash of
  `scripts/build-libmpv-android.sh`; the script writes a `STAMP` (revision plus
  its own hash) last and skips the whole libmpv build when the stamp matches.

Verify with `gh workflow run android.yml -f signed_release=true`, then run it a
second time and check that the libmpv step finishes in seconds.

## 1. Desktop parity

Ordered by value on a phone.

1. **Open share links from outside the app.** The manifest has only the
   launcher intent. Add an `ACTION_VIEW` filter for `drive.proton.me/urls/*` and
   an `ACTION_SEND` (`text/plain`) filter; both open the add-share dialog with
   the link filled in. Desktop: `4a54af3`, `53a1d0a`.
2. **Streaming cache budget.** Android runs on the fixed 4 GiB default
   (`DiskCacheConfig::DEFAULT_BUDGET_BYTES`), large for a phone. Expose
   `DiskCache::set_budget` through `pstr-android`, add a 1–16 GiB choice with a
   usage readout under Storage. Desktop: `a432a93`.
3. **Hardware-decoding switch.** `pstr_mpv.cpp` hard-codes
   `hwdec=auto-copy-safe`; a device with green frames has no way out. A
   Settings toggle, applied from the next file.
4. **Episode list in the player.** A bottom sheet (portrait) or side panel
   (landscape) with the title's episodes, opened on the playing season and
   scrolled to the playing episode. Desktop: `7e734db`.
5. **Long-press menu on tiles.** Play/resume, mark watched or unwatched,
   download, change match, remove from Continue watching, in a
   `ModalBottomSheet`. Desktop: `c5808a5`.
6. **Undo on snackbars** for marking a title and leaving Continue watching.
   `AppUiState.message` is a bare string today; it needs an action. Desktop:
   `79c3f68`.
7. **History tab.** Needs a bridge call over the `pstr-core::library` query
   the desktop page uses. Desktop: `e7eeaa5`.
8. **Buffered range on the seek bar** from mpv's `demuxer-cache-time`.
9. **Up-next card** with the next episode's still and a countdown ring instead
   of "Up next in 7s" text.
10. **Featured banner** at the top of the library. Desktop: `7d03736`.
11. **Bundled Inter** in `ui/theme/Type.kt`, so both clients read the same.
    Desktop: `558446b`.

## 2. UI recommendations

### Everywhere

- **Colours stay the theme's.** The red is the Persona theme's accent and the
  card tint comes from the shared palette, so the redesign changes layout and
  button hierarchy, not colours.
- **One button hierarchy.** At most one filled accent button per screen;
  secondary actions tonal or outlined; destructive actions behind a menu and a
  confirmation, never a filled red button in a list row.
- **Edge-to-edge, per-screen titles.** The "proton-stream" app bar on every
  page wastes a band of height. Library uses a collapsing large title; Title
  pages draw the backdrop under the status bar; other tabs use their name.
- **Skeletons instead of spinners** for library and title loading, and
  generated placeholder art (gradient from the accent plus the title's
  initials) instead of a black card with a play triangle (Akira).
- **Pull-to-refresh** (`PullToRefreshBox`) replaces the top-bar refresh icon,
  with crawl progress shown in the indicator area.
- **Split `ui/ProtonStreamApp.kt`** (1665 lines) into one file per screen
  before the work below; every item touches it.

### Library

- Poster grid (2:3, `posterUrl`) at three columns on a phone, title under the
  poster with no card body. The current 210 dp cropped backdrops fit two titles
  per screen.
- Continue watching: 16:9 still with the progress bar laid over the bottom
  edge of the image and "S03E11 · 12 min left" under it.
- Search becomes an icon in the app bar that expands into an M3 `SearchBar`,
  instead of a permanent field above everything.
- Fix "1 episodes" (line ~507; the season header already pluralises).

### Title page

- Full-bleed backdrop with a gradient scrim, poster, title and meta chips
  (year, score, genres) over it, collapsing as the page scrolls.
- Synopsis clamped to three lines with "More".
- One full-width primary button, "Resume S03E11" or "Play S01E01", then a row
  of icon actions: download, mark watched, and a ⋮ menu holding Start over,
  Change match and More on AniList. Today five buttons of three styles wrap
  over three lines.
- Drop the "Preferred tracks" form and its Save button from this page. The
  player already remembers tracks by name per title (`d3c5f07`); expose any
  override from the player's track chooser.
- Season picker as a dropdown chip or tabs, not stacked accordions; a 40
  episode title is one long scroll today.
- Episode rows: 16:9 still with progress overlay, "1 · Mother and Children"
  (provider name, else "Episode 1") rather than the raw file name
  `[Judas] Oshi no Ko - S01E01.mkv`, runtime or air date as supporting text,
  watched and download state as small trailing icons. The red `S01E01` label
  above each row goes.

### Shares

- `ListItem` rows: name, "12 titles · crawled 2 h ago" or the failure, and a ⋮
  menu with Refresh, Re-enter link and Remove (confirmed). The current row
  wraps "Custom password stored securely" over three lines beside three
  differently styled actions.
- "Add share" as an extended FAB, which is also where the share-link intent
  from parity item 1 lands.

### Settings

- Grouped `ListItem` rows (headline, one line of supporting text, trailing
  switch), with the long explanations moved into the supporting line or an
  info dialog. Today each toggle carries a paragraph.
- Split into sub-pages once storage and decoding settings land: Playback,
  Downloads & storage, Appearance, Metadata, About.
- Language fields become a picker dialog over a language list, not free text.
- Storage shows a usage bar (cache, offline episodes) above the budget choice.

### Robustness

- Screenshot tests (Roborazzi, JVM, no device) for each screen in light and
  dark, phone and tablet, at font scale 1.0 and 1.5 — the redesign changes
  every screen, and `docs/TESTING.md` has no visual layer today.
- 48 dp minimum touch targets and content descriptions on the new icon-only
  actions.
- Tablet: `ListDetailPaneScaffold` for library + title, since
  `NavigationSuiteScaffold` already switches to a rail there.
- Shared-element transition from a poster to its title page, and predictive
  back.

## Suggested order

1. CI green (section 0).
2. Split `ProtonStreamApp.kt`; add screenshot tests for the current screens.
3. Parity 1–3 (small, phone-specific fixes).
4. Button hierarchy and Inter — every later screen inherits them.
5. Title page, then Library, then Shares and Settings.
6. Parity 4–10.
