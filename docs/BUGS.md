# Bugs

The issue ledger. Open items first, then a log of what was fixed and where.

Format: `Bn — one-line summary`, then symptom, cause, fix, and how it was
verified. Reference entries from PRs.

## Open

All of B14–B33 came out of a static review of the Android client
(`android/` and `crates/pstr-android`) on 2026-08-07. **None was reproduced on a
device** — there was no Android acceptance harness to reproduce them with, which
is itself B42. Each states the reasoning that identifies it; treat the
reproduction step as part of the fix. The remediation order is `plan.md` at the
repository root; Phases 1 to 5 of it have landed except for the tail of B41 and
one row of Phase 5's table — everything from B14 to B39 is below in **Fixed**,
and B41 states which of its items are done.

### B41 — assorted correctness and hygiene defects

Individually small, grouped so they are not lost. The items marked **Landed**
came with Phases 4 and 5; the rest are still open, which is why this entry is.
What remains is one item — the asynchronous track-list read-back — plus the
`SettingsStore` construction on the main thread.

- `NativeMpvHost.kt:131-135` reads the track list back immediately after an
  asynchronous property set, so the selection tick lags by up to a second;
  `alang`/`slang` are applied only at load (`pstr_mpv.cpp:225-227`), so a
  language change takes effect one episode late.
- **Landed.** `NativeMpvHost.kt` polled on `Dispatchers.Main.immediate`, and
  `tracks_json` issues five `mpv_get_property` calls per track — 51 synchronous
  property reads on the UI thread for a 10-track file, contending on the core
  lock the demuxer holds during a fetch. The poll loop now runs on
  `Dispatchers.Default` and hops to the main thread only to invoke the state
  listener.
- **Landed.** `PlaybackService` constructed `NativeMpvHost` in `onCreate` with
  a `check(it != 0L)` in the constructor, so a libmpv init failure crashed the
  process and left the graceful "this build does not include libmpv" path
  unreachable. The constructor is private behind `NativeMpvHost.createOrNull()`,
  the service holds a nullable host, and the binder hands back a nullable one —
  which is what `PlayerScreen` was already written for.
- `PlaybackService.kt:43` constructs `SettingsStore` on the main thread inside a
  hot callback; the first SharedPreferences access is disk I/O.
- **Landed.** `MainActivity` set `bound` only in `onServiceConnected`, so an
  activity destroyed with a bind still pending never unbound. It is now set from
  the `bindService` return value.
- **Landed.** The title screen held the only `BackHandler`, so on Shares,
  Downloads and Settings the system back button exited the app. Back from a
  secondary tab now returns to the library.
- **Landed.** `formatBytes` had no KiB branch — a 512 KiB partial rendered as
  `524288 bytes`.
- **Landed.** `ui/theme/Theme.kt` overrode ten roles of `darkColorScheme()` and
  left `surfaceContainer*` (what Card and NavigationSuite draw with) at the
  Material baseline, with no light scheme at all. The scheme is now built from
  the palette `pstr_core::appearance` resolves — the same flavours, accent
  pairings and contrast rule the desktop client uses — so both `surfaceContainer*`
  and a light flavour are covered. Dynamic colour is deliberately still not
  used: the app has a palette of its own and a picker for it, and taking the
  wallpaper's hues instead would override the choice the viewer just made.
- **Landed.** `native_handle()` minted a fresh id per call, so two calls on one
  stream leaked a registry entry permanently. It now remembers the id it
  published under and returns it again.
- **Landed.** `directory_bytes` recursed with no depth limit, so a symlink
  cycle in the cache was an uncatchable stack overflow reached from the settings
  screen. It stops at 16 levels.
- **Landed.** `reject_null_surface` leaked the `jclass` from `FindClass`; it is
  released. `nativeTracks`/`nativeChapters` were declared non-null on the Kotlin
  side, so a null from `NewStringUTF` became an NPE swallowed by `runCatching`
  into a silently empty track list; both are nullable now. `escape()` passed
  4-byte UTF-8 to `NewStringUTF`, which takes Modified UTF-8 — an emoji in a
  track title was mojibake and aborted under CheckJNI. It now decodes the input
  and re-encodes astral characters as surrogate pairs, dropping anything
  malformed rather than passing it on.
- **Landed.** `SettingsStore.kt` duplicated `pstr_core::prefs::PlaybackPrefs`
  (`autoplay` most visibly), which `docs/ANDROID.md:7` says not to do. Playback
  preferences are now read and written over the bridge, and `SettingsStore` holds
  only what is Android's alone — the Wi-Fi-only and background-audio settings.
  `autoSkip` went the other way, into `PlaybackPrefs`, where the desktop client
  can pick it up.
- **Landed.** `ProtonStreamApp.kt` carried five unused imports, which was
  evidence that no lint gate ran. ktlint runs in `check`.

### B42 — nothing verifies any of the above on a device

**Symptom.** Every entry from B14 to B41 was found by reading, because there was
no way to run it.

**Cause.** `android/app/src/test/` held one file, `DownloadCoordinatorTest.kt`,
whose five tests cover pure helpers. There was no `src/androidTest/` directory at
all, so nothing exercised the Keystore, WorkManager, or the UniFFI boundary on
hardware. `scripts/build-android.sh` mapped `check` to `lintDebug
testDebugUnitTest`, and neither `build.gradle.kts` had a `lint { }` block, a
baseline, or ktlint/detekt. `docs/ANDROID.md` mandates a manual matrix — tablet
layout, rotation, process recreation, PiP, download cancel/resume, offline launch
— and `.github/workflows/release.yml:63,164` blocks Android publication until an
"on-device playback matrix" passes. Neither existed as a script; compare
`proton-drive-linux/scripts/fuse-acceptance.sh`. `docs/TESTING.md` did not
contain the word Android.

**Fix, partly landed.** `scripts/android-acceptance.sh` now drives the
device-side matrix, `src/androidTest/` covers the Keystore, the UniFFI boundary
and WorkManager, host tests run under Robolectric, and `check` runs ktlint and a
lint gate with `warningsAsErrors`. Enabling that gate immediately paid for
itself: it reported the four `commit()` calls of
[B20](#b20--queueing-a-season-blocked-the-ui-thread) as `ApplySharedPref` and
the PiP omission of [B40](#b40--picture-in-picture-was-entered-but-never-adapted-to)
as `PictureInPictureIssue`, without being told to look for either. Both have
since landed and both left `lint-baseline.xml`, which is down from 34 entries to
25 and shrinks as the rest land.

**The live cases now exist.** `LiveShareTest` takes the share link as an
instrumentation argument — never a constant, never a file — and covers
`catalog`, `stream-open`, `playback`, `download-cancel-resume` and `watch-state`.
`playback` is the load-bearing one: it drives Proton block storage, decryption,
the Rust stream C ABI, the JNI adapter and libmpv's demuxer, and asserts the
position advances, which can only happen if real decrypted bytes reached the
decoder. One live run serves all five cases, because each re-crawl of a real
share costs about six minutes and the crawl is not what any of them tests.

**Still open, and the reason this entry stays open.** `picture-in-picture` is
`pending`. It is no longer blocked on the app —
[B40](#b40--picture-in-picture-was-entered-but-never-adapted-to) landed, so
there is correct behaviour to assert — but the live suite drives the engine and
libmpv directly, and PiP is a property of the window, so the case needs an
activity-level driver that does not exist yet. Every run still prints that the
matrix is incomplete. `process-recreation` has been promoted off its `xfail`
now that [B35](#b35--navigation-and-player-state-did-not-survive-process-recreation)
has landed, and has not been re-run on hardware since.

**Verified on hardware**, 2026-08-08, Pixel 6 (`arm64-v8a`, API 37) against the
signed minified release APK:

```
passed=13  failed=0  xfailed=1  xpassed=0  skipped=0  pending=1
```

against a real share: every offline case passes, `process-recreation` fails as
the `xfail` for [B35](#b35--navigation-and-player-state-did-not-survive-process-recreation)
predicts, the five live cases pass, and `picture-in-picture` is `pending`. All 20
offline instrumentation tests pass on the release build. `bash scripts/build-android.sh check` passes
with 15 host tests, no new lint findings and no compiler warnings.

The first hardware run also found four defects in the harness itself and five in
the release build; they are [B43](#b43--the-acceptance-harness-could-not-run-against-the-shipping-build)
and [B44](#b44--r8-broke-the-app-under-instrumentation-in-five-places).

## Fixed

### B61 — later seasons were captioned with season one's episode names

**Symptom.** On a 42-title AniList library, Attack on Titan seasons 2–4 and
Jigokuraku season 2 showed season one's episode names. Dan Da Dan, Kaiju No. 8
and The Apothecary Diaries had a handful of names numbered past the end of the
season. Bleach's had 20 rows called `Untitled`. Bleach `S17` (the Thousand-Year
Blood War) and Mushishi `S02`/`S03` had none at all. Jujutsu Kaisen 0 kept the
TV series' names after it was re-pinned to the film. Every desktop tile drew a
blurred strip of AniList banner.

**Cause.** Four, independent.

- `season_episodes` searched for `<title> 2nd Season`, and the scorer, seeing
  the base entry's alias inside the query, picked the base entry again. That
  search was [B11](#b11--seasons-past-the-first-had-no-episode-names-and-were-one-query-from-having-the-wrong-ones)'s
  fix, and it only worked where the sequel's name scored higher than the base's.
- `streamingEpisodes` is what a streaming site published: partial, and
  numbered however that site counted.
- A release's season is not an AniList entry. `S17` is four entries, and
  Mushoku Tensei's `S01` is two.
- An empty episode list was never written, so a re-match kept the old one.
  `bannerImage`, a 4.75:1 strip, was stored as the 16:9 backdrop.

**Fix.** AniList walks `SEQUEL` relations from the match. Each entry's episodes
come from ani.zip with TVDB's season, number and absolute number, plus the
entry's own numbering. `EpisodeGuide::get` meets whichever of those a filename
used without ever crossing seasons. Enrichment is stored as one unit,
replaced even when empty and recorded as asked. A changed match drops the old
entry's rows at once. The backdrop is ani.zip's 1920×1080 fanart, and the
banner is no longer stored. Schema v9 deletes the bad rows. See
[METADATA.md](METADATA.md).

**Verified.** `EpisodeGuide` tests in `pstr_core::metadata` (absolute numbers
in a season folder, by-entry seasons, no season crossing), and the catalog's
`an_empty_enrichment_clears_the_episodes_the_last_match_left`,
`pinning_a_different_entry_drops_the_old_entrys_episodes_at_once` and
`upgrading_drops_anilists_episode_rows_and_banners_but_keeps_its_matches`.
Live, against a copy of the same library through `pstr metadata match`:

- 0 lookups failed.
- Every numbered file is answered except 2 Attack on Titan files, 2 Bleach
  files and one Mushoku Tensei special, which TVDB does not list.
- 29 of 44 matched titles have fanart, and none has a banner.
- A second run asks nothing.

### B60 — a failed "Add and crawl" left the crawl spinner running forever

**Symptom.** Pasting a link without its `#` fragment, or one already added,
and pressing *Add and crawl* cleared the form and put "crawling" in the top
bar, where it stayed until the app was restarted. The Refresh button never
came back. The only sign of the real problem was a status line that had
already faded.

**Cause.** `Action::AddShare` set `crawling` and cleared the form before the
engine had answered. `Engine::add_share` reported a refused link through
`fail`, which sends a plain `Error`, so nothing ever sent `CrawlFinished` for
a crawl that never started.

**Fix.** The engine answers with `ShareAdded` or `ShareRejected`, and the
form keeps its contents until one of them arrives. A rejection clears
`crawling` and shows its reason inside the form. The link is also checked
while it is typed, using `pstr_core::shares::share_token`, the same check
`ShareStore::add` runs first, so most refusals never reach the engine.

**Verified.** `ui::shares::tests`: `a_link_without_its_key_is_refused_while_typing`
and `a_share_already_added_is_refused_while_typing`.

### B59 — every poster fell back to initials after each match run

**Symptom.** After *Match the library*, and after picking or forgetting a
match by hand, every tile in the grid dropped to its initials and faded the
same picture back in.

**Cause.** `Event::Metadata` cleared the whole poster cache, because any
title's record might have changed.

**Fix.** `app::changed_art` compares each title's tile-art URL before and
after, and only those titles are forgotten. A title whose URL did not change
keeps its texture.

**Verified.** `app::tests::a_match_run_that_changes_nothing_keeps_every_poster`
and `only_titles_whose_art_moved_lose_their_poster`.

### B58 — "Nothing in the library yet" flashed at every launch

**Symptom.** For the fraction of a second it took to read the catalog, the
library page showed the empty-library message and its *Add a share* button,
including for a library with hundreds of titles.

**Cause.** `App::library` starts as `Library::default()`, which is empty, and
the page could not tell "not read yet" from "nothing in it".

**Fix.** `App::loaded` is set by the first `LibraryLoaded`. Until then the
page draws two rows of blank tiles in the shape of the grid.

### B57 — subtitles drawn under the player's controls

**Symptom.** While the controls were up, the bottom subtitle line sat
behind the seek bar and the time labels and could not be read. The controls
are up whenever the mouse moves, which is when someone is watching.

**Cause.** mpv positions subtitles against the whole window. Nothing told it
that the bottom strip of the window was covered.

**Fix.** `Playback::lift_subtitles` sets mpv's `sub-pos` from the measured
height of the controls while they are visible, and back to 100 when they
hide. It sends a command only when the value changes. `sub-pos` rather than a
margin, because it also moves ASS subtitles.

### B56 — horizontal lines across the player's scrims

**Symptom.** Thin dark lines ran across the picture in the fade under the
title strip and the fade above the controls.

**Cause.** Each scrim was a stack of flat bands, each one pixel taller than
its step so no gap could open between them. Every overlap was a line drawn
at twice the alpha.

**Fix.** `ui::player::fade` draws one mesh with a colour per row of vertices,
and the GPU interpolates between them. There are no overlaps left to show.

### B55 — every newly fetched block waited for its own fsync before playback

**Symptom.** Not visible as a bug. Every block the player had to wait for
from the network, which means the first frame and the block after every
seek, also waited for a 4 MiB write to the block cache, an fsync of it, and an
fsync of its sidecar.

**Cause.** `block_at` awaited `DiskCache::put` before returning the block, and
`write_entry` synced both files.

**Fix.** The put runs in a spawned task behind the read. The sidecar is no
longer synced: the ordering the module note describes depends only on the
block's sync, and a sidecar lost in a crash only turns a valid entry into a
miss. Downloads now open their streams with `StreamSource::open_for_copy`,
which reads the disk cache but never writes to it. A download no longer
pushes the episode just watched out of the cache to store bytes that are about
to be on disk anyway.

**Verified.** `stream::tests::a_fetched_block_reaches_the_disk_cache_without_the_read_waiting_for_it`
and `a_copying_stream_reads_the_disk_cache_but_never_adds_to_it`.

### B54 — a window on another workspace was declared "not responding"

**Symptom.** On Hyprland, switching to another workspace while `proton-stream`
was playing brought up the compositor's "application is not responding" dialog
a few seconds later, offering to wait or to kill it. Choosing *wait* dismissed
it for good, and coming back to the workspace found the window working normally.
Wayland only; the app was never actually broken.

**Cause.** A Wayland compositor throttles a client by withholding the
`wl_surface` frame callback, and withholds it entirely while the surface is not
being shown. Mesa's EGL implements a swap interval of 1 by waiting on that
callback, so `eglSwapBuffers` on a hidden surface never returns:
`run_ui_and_paint` → `glutin swap_buffers` → `eglSwapBuffers` →
`wl_display_dispatch_queue` → `ppoll`, with no timeout. That is the UI thread,
and the UI thread is what answers `xdg_wm_base.ping`, so the compositor stops
hearing from the app and puts up the ANR dialog. It takes *sustained* repaints
to hit — the throttle waits on the previous frame's callback, so the first swap
after the window is hidden still returns — which is why it showed up during
playback, where mpv asks for a frame per picture, and not on an idle window.

eframe already skips painting a viewport it believes is hidden, but nothing
tells it: `WindowEvent::Occluded` is X11, macOS and web only — winit removed the
Wayland implementation in 0.29 — so `ViewportInfo::occluded` stays `None`.

**Fix.** `pstr-app::pacing`. On Wayland the swap interval is set to 0
(`NativeOptions::glow_options.vsync`), which makes `eglSwapBuffers` return
whether the surface is being shown or not, and `Pacer` caps the frame rate in
vsync's place — 60 fps by default, `PSTR_FRAME_CAP` to change it, `0` to remove
the ceiling. Nothing is lost by dropping vsync there: a Wayland compositor
composites from the last buffer a client committed, at its own refresh, so a
client that commits too often wastes work but cannot tear. X11 and Windows are
untouched and keep vsync. The cap is a ceiling and not a rate, so an idle window
still draws nothing at all.

**Verified.** Reproduced on Hyprland 0.56.2 with a minimal eframe 0.35 client
that repaints continuously: moved to a hidden workspace with `hyprctl dispatch
movetoworkspacesilent`, its frame counter stopped dead and `gdb` put thread 1 in
`eglSwapBuffers` → `wl_display_dispatch_queue`. The same client with the swap
interval at 0 kept drawing while hidden and stayed in the event loop. The app
itself was then run hidden for 30 s: it stays in `calloop`'s poll, and steady
state costs 0% of a core, so the ceiling has not turned an idle window into a
busy one. `cargo test -p pstr-app pacing` covers the cap.

### B53 — scanning a new library outran AniList, and the crawl waited on one folder at a time

**Symptom.** Two slow starts, both on the first scan of a share. Matching a
library against AniList worked for the first couple of dozen titles and then
returned nothing but failures for the rest of the run; because a failure is
deliberately not cached, the next run started over and failed in the same place,
so a large library never finished matching. Separately, the crawl itself took
minutes on a share whose contents are a few hundred nodes.

**Cause.** Two independent ones.

- Nothing paced provider requests. A scan sends one search per title, a second
  search for a name that needs its apostrophe repaired, one episode request per
  match and up to three more per further season — several hundred requests as
  fast as two workers could issue them, against an API that allows ninety a
  minute and has been running in a degraded mode that allows thirty. Everything
  past the window got a `429`, which `AniList::query` turned straight into
  `Error::RateLimited` with no wait and no retry.
- `SharedLibrary::crawl` walked the tree one folder per round trip. A media
  share is wide, not deep — a folder per series, a folder per season under it —
  so a few hundred sibling folders were listed strictly in sequence. The SDK
  already fans its detail batches out within one folder; nothing overlapped
  across folders, and the crawl was almost entirely latency.

**Fix.** `pstr-meta::limiter` is a shared, self-tuning pacer: requests take a
slot before they are sent, `X-RateLimit-Limit` on any answer retunes the rate to
what the provider currently allows, and a `429` parks *every* lookup in flight
for as long as `Retry-After` asks (capped at 30s) and then retries, twice, before
giving up as `Error::RateLimited`. One limiter lives inside each provider, which
lives in an `Arc` inside `MetadataService`, so the per-title clones share it.
TMDB routes through the same code — its search request now goes through the same
paced `get` as everything else rather than building its own.

`SharedLibrary::crawl` is now breadth-first and lists `CRAWL_CONCURRENCY` (6)
folders of a level at once. `buffered`, not `buffer_unordered`: two identical
crawls should produce the same order. The client clones share one node-key cache
and one single-flight map, so siblings needing the same ancestor key still wait
on a single derivation.

**Verified.** `cargo fmt --all -- --check`, `cargo clippy --workspace
--all-targets -- -D warnings`, `cargo test --workspace --locked`. The limiter's
pacing, its `429` parking and its retuning from headers are covered by unit tests
on a paused clock (`crates/pstr-meta/src/limiter.rs`).

### B52 — "paint the accent as a gradient" did nothing on Android

**Symptom.** The appearance page on Android offers a *Paint the accent as a
gradient* switch, stores it, and repaints the app when it is flipped — and
nothing on the screen changed either way. The same stored setting visibly
changed the desktop client, so one file meant two things depending on which
client read it. `PinkSky`, whose whole point is that it is a gradient before it
is a colour, arrived on Android as one flat pink.

**Cause.** No gradient was ever drawn. Nothing under `android/app/src/main`
referenced `Brush`, `linearGradient` or `verticalGradient`. `accent_alt` reached
Compose only as `secondary`, where it fills tonal containers — so the second hue
existed in the scheme but never sat next to the first one.

**Fix.** `ui/theme/Accent.kt` draws the ramp on the surfaces the desktop draws it
on: the primary button, the seek bar, the watch-progress bar under a tile and
under an episode row. Nothing there reads the toggle, because it does not need
to — `Palette::resolve` already sets `accent_alt` equal to `accent` when
gradients are off, so the ramp collapses to a flat fill by itself and the two
clients cannot disagree about what the setting means.

Two surfaces are deliberately not ramps. A download in flight keeps a solid
`tertiary`, because it is drawn directly under the watch-progress bar of the same
row and two ramps in the same colour two pixels apart say nothing. The navigation
pill is solid `primary`: `NavigationSuiteScaffold` takes its indicator as a
`Color` with no slot to draw into, so a gradient there would mean reimplementing
the bar — but the accent itself is still nearer the desktop's rule than
`secondaryContainer`, which is the accent taken most of the way back to the page.

**Verified.** `./gradlew :app:testDebugUnitTest :app:lintDebug`. Still wants a
device: the ramp is the one thing here a screenshot on Proton and on Latte
settles, and the panel-banding case the toggle exists for cannot be judged from
a build at all.

### B51 — the Android client wore only part of the palette it was given

**Symptom.** Every flavour looked half-applied on Android. The page was the
flavour's colour and the tiles on it were not: the library grid, the
continue-watching row, every episode row, every share, every download and the
mini transport were all a flat neutral grey that belonged to no flavour and did
not change when one was picked. Latte additionally drew white status-bar icons
onto its near-white page, and every cold start flashed a dark blue-grey before
the app appeared — including for viewers who had chosen Latte.

**Cause.** Four, all in how the resolved palette reached the platform.

`schemeOf` (`ui/theme/Theme.kt`) built a `ColorScheme` by copying
`darkColorScheme()`/`lightColorScheme()` and overriding 22 roles. Material names
roughly fifty. A role nobody overrides keeps Material's own baseline value, and
`Card`'s container is `surfaceContainerHighest`
(`FilledCardTokens.ContainerColor`), which was one of the twenty-seven nobody
overrode. `Card` is what draws every repeated surface in the app, so the app's
most common surface was the one surface that ignored the theme. `inverseSurface`
and `inverseOnSurface` — the snackbar, which is the client's only error channel
— and `scrim`, the dim behind every dialog, were in the same set. `surfaceTint`
was too, so raised surfaces were tinted with Material's baseline purple.

`themes.xml` hardcoded `windowBackground` to `#1E1E2E`, which is Catppuccin
Mocha's base: not the shipped default, and not the viewer's choice either. It
also pinned `windowLightStatusBar` and `windowLightNavigationBar` to `false`,
and nothing anywhere synced either to `palette.light`.

`ProtonStreamTheme` read the palette in a `LaunchedEffect` that hopped to
`Dispatchers.IO`, so the first composition was always painted in a hardcoded
dark default — the same flash-of-the-wrong-theme the desktop client had on
Windows, for the same reason.

**Fix.** `schemeOf` maps every role Material names. Two colours the palette does
not name are derived in Rust so the blend behind them is the shared, tested one:
`elevated`, which continues the flavour's own `card` → `card_hover` step by
extrapolating past it — so a light flavour's ladder descends and a dark one's
climbs — and `danger_dim`, an error *container* rather than error ink. Both are
new fields on `PaletteRecord`. `outline` and `outlineVariant` were also swapped
to match what Material draws with each: the visible border takes `muted`, the
divider takes the fainter `border`. `surfaceTint` is transparent, because the
desktop draws no elevation tint and its rule is that the accent is the only
strong colour in the window.

A new `stored_palette` bridge function resolves the stored choice without
building an engine — no SQLite, no Tokio runtime, no Keystore unlock — so
`MainActivity.onCreate` can seed the palette before `setContent` rather than one
frame after it. It also sets the window background and the system bars' icon
polarity from that palette, and re-applies both whenever the flavour changes, so
picking Latte turns the status-bar icons dark in the same frame the page turns
light. `themes.xml` keeps only the shipped default's background, as a first-ever
launch has no theme file to read.

**Verified.** `cargo test --workspace --locked`, `cargo clippy --workspace
--all-targets -- -D warnings`, `./gradlew :app:testDebugUnitTest :app:lintDebug`
(lint clean; the two unmatched baseline entries it reports pre-date this change
— the same run on the unmodified tree reports them). The Kotlin side is pinned
by `SchemeRolesTest`, which sweeps `ColorScheme` reflectively rather than from a
list and fails if any role carries a value the palette did not supply, so a role
added by a future Material version fails the day the dependency moves. The Rust
side asserts the derived rung continues the ladder in all five flavours, both
polarities. The appearance itself still wants a device: screenshots on Proton
and on Latte.

### B50 — every episode of a show was named after the show

**Symptom.** All fifty-nine Attack on Titan rows read `Attack On Titan` on both
lines — no numbering, nothing to tell one from the next. Oshi no Ko, in the same
library, was fine.

**Cause.** `build_rows` treats a filename title that differs from the folder's
as the *episode's* own name (`catalog.rs`), which is what turns
`Neon Genesis Evangelion - Death & Rebirth.mkv` under an `NGE` folder into an
episode called `Death & Rebirth`. Both the comparison and the prefix strip were
case-sensitive, and the two names come from different hands: the folder was
`Attack on Titan`, the release group wrote `Attack On Titan`. One letter made
them "different", the strip then found no prefix to remove, and the whole series
name was stored as `episode_title` — which `Episode::label` and
`Episode::detail` both prefer over the numbering.

**Fix.** Both are case-insensitive, and a strip that removes nothing no longer
yields an episode name.

**Verified.** Two table tests in `catalog.rs`; `cargo test --workspace`. Note
that the wrong `episode_title` is *stored*, so an existing catalog needs a
re-crawl (the library's refresh) before rows correct themselves.

### B49 — a recoverable decoder fallback put a dialog over a playing episode

**Symptom.** Every HEVC episode opened with
`hevc_mediacodec: Both surface and native_window are NULL` over the picture, and
then played normally.

**Cause.** Two things, both exposed by B46 finally letting the MediaCodec
decoders initialise. FFmpeg's *direct* MediaCodec decoders render into an
`ANativeWindow` handed to them at init; this player has none to give — the
picture goes through `vo_libmpv` on an EGL pbuffer, and the SurfaceView attaches
later than the load, if at all (background audio). `hwdec=auto-safe` picked them
regardless. Separately, `record_log` promoted *any* error-level mpv log line to
the UI, and libavcodec logs a decoder that refuses to initialise as an error
even where mpv treats it as a fallback.

**Fix.** `hwdec=auto-copy-safe` — the copy variants decode in hardware and hand
back ordinary frames, which is what this render path can use. And `record_log`
ignores the `ffmpeg/…` prefix: mpv reports the failures that are actually fatal
under a prefix of its own.

**Verified.** `gradlew externalNativeBuildDebug`.

### B48 — tonal buttons were grey shapes on a grey page

**Symptom.** Download, Change match, Pause, the navigation bar's selected pill —
every tonal control was a dark grey rounded rectangle a shade off the card under
it. Readable as text, invisible as a control. Episode rows compounded it: forty
rows each arranged themselves around whatever their own download state needed,
so no two lined up.

**Cause.** `Theme.kt` mapped Material's `secondaryContainer` — which fills every
`FilledTonalButton` and the navigation indicator — to the palette's `cardHover`,
one step off `card`. And `EpisodeRow` gave each row a second line of buttons
whose widths depended on the row's own state.

**Fix.** `secondaryContainer` is `accentDim`, the accent taken back towards the
page, which is what the role is for. The episode row is one row: the whole card
plays, the numbering leads the text column, and the two trailing controls sit in
fixed-width slots — including an empty one — so every row aligns. Numbering is
always printed, falling back to the season folder's number and then to the row's
position, so rows the parser could not number still differ. Season headers carry
a count and an icon-only download. The page also ends above the floating mini
transport instead of behind it.

**Verified.** `gradlew compileDebugKotlin`.

### B46 — every hardware decoder failed, and playback ran in software

**Symptom.** Starting an HEVC episode on Android raised
`hevc_mediacodec: No Java virtual machine has been registered` and played
anyway — decoded on the CPU, with the heat and dropped frames that implies on
anything above 1080p.

**Cause.** FFmpeg's MediaCodec decoders reach Android's Java API through a
`JavaVM` handed to them by `av_jni_set_java_vm`; there is no way for them to
find one themselves. mpv-android registers it in its own JNI layer
(`app/src/main/jni/main.cpp`), and `pstr_mpv.cpp` replaces that layer, so
nothing ever called it. `hwdec=auto-safe` then failed to open every MediaCodec
decoder and fell back to software.

**Fix.** `JNI_OnLoad` in `pstr_mpv.cpp` registers the VM — the loader calls it
once per process, before any native method and long before a decoder runs.
libmpv links FFmpeg dynamically without re-exporting it, so `CMakeLists.txt`
links `libavcodec.so` (already packaged as part of libmpv's staged dependency
closure) and `scripts/build-libmpv-android.sh` stages `libavcodec/jni.h`
alongside the mpv headers.

**Verified.** `gradlew externalNativeBuildDebug`; `libpstr_mpv.so` exports
`JNI_OnLoad`, imports `av_jni_set_java_vm@LIBAVCODEC_63` and gains a
`libavcodec.so` `DT_NEEDED`.

### B47 — episode labels were set one character per line

**Symptom.** On a phone, an episode row rendered "S03E01" down the screen a
letter at a time, and the show screen did the same to "More on AniList".

**Cause.** Both were fixed `Row`s in which the flexible child came last: the
episode row gave a 96 dp still, a Play button, a watched toggle and a download
button their intrinsic widths and left `Modifier.weight(1f)` whatever remained,
which on a narrow screen is a few pixels. Compose does not drop a child to make
room; it wraps the text into the column it was given.

**Fix.** The episode row is two rows — still plus label above, controls below —
with the label ellipsized rather than wrapped without bound. The show screen's
two button rows are `FlowRow`s, so a button that does not fit moves to the next
line instead of squeezing its neighbour. The season header gives its label the
weight so a long season name cannot squeeze "Download season".

**Verified.** `gradlew compileDebugKotlin` and `ktlintMainSourceSetCheck`.

### B36 — an invalidated connection stayed live, and releasing a stream could no-op

**Symptom.** Removing a share left its authenticated visitor client usable for
the life of the process; adding a share made the next episode start pay a cold
handshake for every configured share.

**Cause.** `invalidate_connection` only bumped an `AtomicU64`. The cached
`Connection` — including the live `ProtonDrivePublicLinkClient` for the removed
share — stayed in `self.connection` until something next called `connection()`.
Because the counter was process-wide rather than per-share, the next
`connection()` re-handshaked all of them. The same counter made `release_stream`
silently skip `source.close` whenever a share was mutated while the player was
open, and it returned `()`, so Kotlin could not tell.

**Fix.** `invalidate_connection` now takes the share id that changed, drops the
cached connection there and then, and parks every *other* share's client for the
next build — `SharedLibrary::open_all_reusing` takes them, so adding one link
re-handshakes one link. The removed share's client is the one client not carried
over, which is what makes it unreachable rather than merely stale. The cache
slot moved to a sync mutex, with the open handshakes serialised behind a
separate `connecting` lock, because the mutations that invalidate it are sync
`&self` methods called from the JNI thread. `release_stream` closes against
whatever connection is cached, regardless of generation, and returns
`Result<(), BridgeError>`.

**Verified.** `cargo test --workspace`, `cargo clippy --workspace --all-targets`.
Not exercised on a device: the reuse path needs two configured shares and a
network.

### B37 — one provider error abandoned the rest of the library, and episode metadata was never fetched

**Symptom.** Matching reported a failure and left most of the library
unenriched; Android never showed provider episode titles at all.

**Cause.** `match_titles` ran serially and propagated the first error with `?`,
where the desktop equivalent fans out under a semaphore and deliberately counts
failures rather than aborting. The bridge also had no analogue of desktop's
`Work::Episodes`/`episodes_of`, so `Catalog::set_episode_metadata` was never
populated and `all_episode_metadata` was dead from Android's side.

**Fix.** `match_titles_inner` is desktop's `run_match` on a `JoinSet` under a
two-permit semaphore: the same two kinds of work (search, or episodes for an
already-matched title), the same "misses are stored, failures are not" rule, and
a `MatchSummary` of matched/unmatched/failed/episodes returned across the bridge
and shown to the viewer. `episodes_of` moved out of `pstr-app` into
`MetadataService::title_episodes` so both clients call one implementation rather
than two copies. `library()` now reads `all_episode_metadata` and
`EpisodeRecord` carries the provider's name, overview, still and air date; the
episode row shows the provider's name where there is one and the parsed filename
detail otherwise.

**Verified.** `cargo test --workspace` (desktop's matching tests cover
`title_episodes` unchanged), `bash scripts/build-android.sh check`. The fan-out
and the failure counting are not covered by an offline test — both need a
provider.

### B38 — the library was rebuilt in full on every keystroke, and the downloads screen composed eagerly

**Symptom.** Search was sluggish on a large library; the Downloads tab froze on
first frame with many offline episodes.

**Cause.** `library()` ran four table scans, a `std::fs::metadata` per offline
file, a `Library::build` and a full record conversion before the search argument
was applied as a filter, and `AppViewModel` called it per keystroke. It also
*wrote* — `remove_offline_file` — from what read as a query. On the Kotlin side
the Downloads screen was a `Column(verticalScroll)` with `forEach`, not a
`LazyColumn`.

**Fix.** The whole conversion is cached behind `Catalog::writes()`, which is
SQLite's own `total_changes` — a counter no write path has to remember to
invalidate, which is the failure mode a hand-maintained one has. A search is now
a filter over records that already exist. The pruning of offline rows whose
bytes are gone moved into `prune_offline_files()`, called once per full reload
rather than per keystroke, which is also what makes `library()` cacheable at
all. Downloads is a `LazyColumn` keyed by share and link id.

**Verified.** `cargo test --workspace`, `bash scripts/build-android.sh check`.
The improvement itself is a timing claim and has not been measured on a device.

### B39 — a release build logged at trace, and a TLS init failure was discarded

**Symptom.** Shipped APKs wrote rustls handshake internals and everything else
in the dependency graph to logcat. A platform-verifier initialisation failure
surfaced much later as an unexplained certificate error.

**Cause.** `initTls` called `android_logger::init_once` with `LevelFilter::Trace`
and no `cfg!(debug_assertions)` guard, discarded the result of
`rustls_platform_verifier::android::init_with_env`, and returned `void`, so
Kotlin could not detect it either.

**Fix.** Debug in a debug build, Info in a release one. `initTls` returns a
boolean, logs the verifier error at `error` level, and `NativeRuntime.tlsReady`
carries the answer to the ViewModel, which tells the viewer that requests will
fail instead of leaving them to discover it per share.

**Verified.** `bash scripts/build-android.sh check`. The failure branch cannot
be provoked without a broken platform verifier.

### B24 — playback was structurally impossible without a video surface

**Symptom.** A load issued while the surface was destroyed — background audio,
or the screen off — stalled five seconds and then failed with `libmpv rejected
the stream or no video surface was available`.

**Cause.** `Player::load` waited on `render_cv_` for a non-null `render_` with a
5 s timeout, and `render_` was only ever created from the surface-attach path.
There was no fallback. So the one setting whose entire purpose is playing
without a visible window could not work, by construction.

**Fix.** EGL and the mpv render context are created with the render thread and
need no window at all — a 1×1 pbuffer is enough to make a context current, and
`vo_libmpv` only needs *some* consumer. Attaching a surface then adds a window
surface to a context that already exists, and `load` waits for nothing.

**Verified.** Compiles for both ABIs. Needs the background-audio acceptance case.

Not changed, and deliberately: `withOpenHandle` still holds its lock across the
whole JNI call. That lock is what serialises `nativeDestroy` against every other
entry point, and with the five-second wait gone there is no longer a JNI call
that blocks long enough for holding it to matter — `nativeDestroy` itself now
runs on its own thread (B16).

### B25 — frames were not drained while the surface was detached

**Symptom.** After the screen went off, or after PiP tore down, video came back
desynchronised from audio rather than resuming cleanly.

**Cause.** `render_update` kept setting `frame_ready_`, but
`mpv_render_context_update`/`_render` were only called when a window surface
existed. `vo_libmpv` expects its host to keep draining; with no consumer the
video thread stalls.

**Fix.** With no window the render loop makes the pbuffer current and renders
with `MPV_RENDER_PARAM_SKIP_RENDERING`, which consumes the frame and drops it.
The frame is accounted for either way, which is all mpv asks.

**Verified.** Compiles for both ABIs. Needs a device to observe the sync.

### B26 — the seek bar issued a seek per drag pixel

**Symptom.** Dragging the seek bar on a network stream was far slower to settle
than a single tap-to-seek, and the thumb visibly snapped backwards under the
finger.

**Cause.** The `Slider` had no drag-local state and no `onValueChangeFinished`,
so every touch-move called `nativeSeek`. Each seek aborts every outstanding
prefetch, so a one-second drag issued dozens of seeks and threw away every
in-flight block — and the thumb was drawn from `state.position`, which only
updates four times a second.

**Fix.** The drag position is held locally and drives both the thumb and the
elapsed-time label; one seek is issued on release.

**Verified.** Compiles and passes `check`.

### B27 — a network-backed stream gave the viewer no feedback

**Symptom.** When the stream stalled the picture simply froze. When playback
failed after `loadfile`, nothing was shown at all.

**Cause.** Nothing observed `paused-for-cache`/`cache-buffering-state` and
nothing handled `MPV_EVENT_SEEK`/`MPV_EVENT_PLAYBACK_RESTART`, so there was no
buffering state to render — desktop has had one all along
(`pstr-app/src/ui/transport.rs`). The error slot caught setup exceptions only:
no `MPV_EVENT_LOG_MESSAGE` and no end reason reached the UI, and the message
could not be dismissed.

**Fix.** Both cache properties are observed and both seek events handled, giving
`MpvPlaybackState.stalled` — the picture is stopped for a reason the viewer did
not choose — which draws a spinner and, while buffering, the cache percentage.
mpv's own error log is requested at `error` level and the *first* line about the
open file is kept: a failure cascades, and the first line is the one that names
the cause. It is consumed by reading, cleared by the next load, and reaches the
same error slot, which is now dismissible — a subtitle track that failed to load
leaves an episode that plays perfectly well behind what used to be a permanent
message.

**Verified.** Compiles for both ABIs and passes `check`. The stall path needs a
throttled link on a device.

### B33 — audio focus was handled halfway, and unplugging headphones played out loud

**Symptom.** After any interruption playback stayed paused until the user
manually resumed. Unplugging headphones continued playback on the speaker.

**Cause.** `AUDIOFOCUS_LOSS_TRANSIENT` paused and `AUDIOFOCUS_GAIN` was
explicitly `Unit`, so nothing ever resumed; `AUDIOFOCUS_LOSS` did not abandon
the request; the `requestAudioFocus` result was discarded, so playback proceeded
even on `AUDIOFOCUS_REQUEST_FAILED`. There was no `ACTION_AUDIO_BECOMING_NOISY`
receiver anywhere in the module.

**Fix.** The four cases are treated as the four different things they are: a
transient loss pauses and remembers that *it* did — so a viewer who had already
paused is not resumed for them — and gain resumes only in that case; a duckable
loss drops the volume and restores the remembered one rather than a re-read
ducked one; a permanent loss abandons the request instead of holding it; and a
refused request pauses rather than playing over whatever holds focus. A
non-exported `becomingNoisy` receiver pauses on the jack being pulled.

**Verified.** Compiles and passes `check`. Every case needs a device.

### B34 — a partial EGL failure was permanent, silent and undiagnosable

**Symptom.** A black screen that persisted for the life of the process.

**Cause.** `initialize_egl` assigned `display` before it could still fail at
`eglChooseConfig`, `eglCreateContext` or `eglCreatePbufferSurface`. The retry
guard on the next attach was `display == EGL_NO_DISPLAY`, which was now false,
so initialisation was never retried — and every later attach used an
uninitialised `config` and `EGL_NO_CONTEXT`. No `eglGetError` was checked or
logged anywhere in the file.

**Fix.** Initialisation is all-or-nothing: any failing step unwinds everything
before it and restores the sentinels, so the guard stays honest, and logs
`eglGetError` with the name of the step that failed. Landed early, with B24 —
the two are the same function and B24's restructure depends on "is there a
display" meaning what it says.

**Verified.** Compiles for both ABIs. The failure path needs a device that
actually fails.

### B35 — navigation and player state did not survive process recreation

**Symptom.** Toggling dark mode, changing font size, or returning after the
process was killed dropped the viewer back on the Library tab with playback
gone. Closing the player reset the title page's scroll and expanded seasons.

**Cause.** The destination, the selected title, the play request and the
player's episode index were plain `remember`, and nothing was written to
`onSaveInstanceState`. The `configChanges` list covered orientation and screen
size but not `uiMode|density|fontScale|locale|layoutDirection`. And when a play
request existed the composable *returned* before the scaffold, disposing the
whole subtree, so the title page was rebuilt from the top on exit.

**Fix.** What is saved is keys, not records: which title and which episode. A
`TitleRecord` is neither parcelable nor still current after a library reload, so
everything else is derived from `state.titles` — which also means a reload that
renames a season updates the open screen instead of pinning a stale copy. The
player's index is hoisted to the same place for the same reason.
`configChanges` covers the five missing configurations, and the player is drawn
*over* the scaffold rather than instead of it, so leaving it finds the title page
where it was.

**Verified.** Compiles and passes `check`. The `process-recreation` acceptance
case is `xfail` against this bug — it should now be promoted, which needs a
device run to confirm.

### B16 — tearing down the player could block the main thread for a whole block fetch

**Symptom.** Closing the player or stopping the service on a degraded connection
froze the UI; a seek issued during a stalled read did not take effect until the
read completed.

**Cause.** `~Player` calls `mpv_terminate_destroy`, which joins mpv's demuxer
thread — and that thread may be parked inside `stream_read` →
`read_range_for_native` → `runtime.block_on(stream.read_range(...))` on a 4 MiB
Proton block. `stream_cancel` only set an atomic that `stream_read` checked at
*entry*, so an in-flight read was not interruptible and mpv's `cancel_fn` never
reached Rust at all. `pstr-stream` has had a working cancellation story since
[B6](#b6--read-ahead-starved-the-seeks-it-was-supposed-to-smooth); the Android
path could not reach any of it. And `close()` ran the whole teardown on the main
thread, from `PlaybackService.onDestroy`.

**Fix.** `pstr_android_stream_cancel` is a new C ABI entry point that trips a
`tokio::sync::Notify` the in-flight `read_range` is selected against, and
`stream_cancel` calls it. Only a waiting read is affected — a cancel with no
reader is dropped — which is what makes it safe to call speculatively.
`nativeDestroy` moved to a named teardown thread; `closed` is set first so
nothing new enters the handle, and the lock is held only to let a call already
inside finish.

**Verified.** Compiles for both ABIs. The latency claim needs the throttled-link
acceptance case.

### B20 — queueing a season blocked the UI thread

**Symptom.** "Download show" on a long series froze the app, up to an ANR.

**Cause.** `DownloadStateStore` used blocking `.commit()` and
`DownloadCoordinator` looped it once per episode, from UI click handlers. A
200-episode series was 200 synchronous fsyncs plus 200 `enqueueUniqueWork` calls
on the main thread.

**Fix.** Every write is `apply()` — the in-memory map is updated before it
returns, so a read that follows a write in the same process still sees it — and
a queue is one `putAll` rather than one write per episode. Bulk enqueue runs on
the coordinator's own IO scope.

**Verified.** `check` passes, and the four `ApplySharedPref` entries left the
lint baseline.

### B21 — cancelling a coroutine did not cancel the Rust work behind it

**Symptom.** Backing out of a screen left provider requests and SQLite writes
running; a cancelled download kept downloading and kept calling back into a
`DownloadObserver` WorkManager considered dead.

**Cause.** Every async export had the shape `runtime.spawn(async { … }).await`.
UniFFI cancellation drops the Rust future, and dropping a `JoinHandle`
*detaches* the task rather than aborting it.

**Fix.** All eight exports go through `spawned(&runtime, …)`, which wraps the
handle in an `AbortOnDrop` that aborts on the way out. Abort points are await
points, so a blocking SQLite statement always completes; what is abandoned is
the work after it.

**Verified.** Compiles and passes clippy; the observable half needs the
cancelled-download acceptance case.

### B22 — playback and downloads never refreshed an expired visitor session

**Symptom.** After the app sat in the background for hours, playing an episode
failed with an authentication error, and the only way out was a pull-to-refresh.

**Cause.** `SharedLibrary::refresh_session` was called only from `crawl_inner`.
`open_stream_inner` and `download_episode_inner` went straight to
`connection().source.open(...)` with no refresh and no retry, and the generation
counter invalidates on share-store mutations only — never on session age or an
auth failure.

**Fix.** Both open paths go through `open_from_source`, which has two guards
because there are two ways to lose a session. Age is the common one, caught
before the request by a per-share timestamp with a five-minute window; anything
else only shows up as a failure, so one retry behind a refresh covers it. A
refresh that itself fails is not reported — the open's own error is more useful.

**Verified.** Compiles. Needs a live session to actually expire.

### B23 — read-ahead outlived the stream it was reading for

**Symptom.** Closing the player mid-episode kept consuming data, and evicted
blocks the *next* stream had already fetched.

**Cause.** There was no `impl Drop` anywhere in `pstr-stream`.
`VideoStream::read_ahead_from` spawned detached tasks each holding an
`Arc<Inner>`, and `cancel_read_ahead` ran only on a detected seek.
`StreamSource::close` popped the LRU entry and called `ring.forget`, but the
in-flight prefetches kept running and re-inserted into the ring the `forget` had
just cleared.

**Fix.** Prefetch tasks hold a `Weak<Inner>` and upgrade only for the block they
are fetching, so a queue of speculation no longer keeps its own stream alive.
`Drop for Inner` then aborts whatever is left, and `StreamSource::close` cancels
explicitly *before* clearing the ring, so nothing can re-insert into it. The
residue is the one block a prefetch had already upgraded for; the window behind
it is not.

**Verified.** `abandoning_a_stream_stops_its_read_ahead` in
`crates/pstr-stream/src/stream.rs` — drops a stream with prefetches in flight
against a slow source and asserts at most one more fetch lands.

### B29 — the Wi-Fi-only setting did not reach work already queued

**Symptom.** Queue a season with the toggle off, turn it on, leave the house —
the queue downloaded over cellular.

**Cause.** `Constraints` were baked in from `SettingsStore(context).wifiOnly` at
enqueue time, and nothing re-enqueued pending work when the setting changed.

**Fix.** `DownloadCoordinator.applyNetworkPolicy` re-issues every queued or
running download with the current constraint, and the toggle calls it. Running
work needs no special handling: WorkManager stops it once the new constraint
stops being met, and the `.part` file makes that a pause rather than a loss.

**Verified.** Compiles and passes `check`.

### B30 — a partial download could become unreclaimable

**Symptom.** Settings reported several GiB of unfinished downloads with no way
to delete them short of clearing app data.

**Cause.** `remove_all_offline` iterated `catalog.all_offline_files()` —
*completed* downloads only — so a `.part` with no catalog row was never touched.
Nothing checked free space before enqueuing, and the work constraints set
neither `setRequiresStorageNotLow` nor anything else, so ENOSPC was reached by
writing to it.

**Fix.** "Delete all offline" now sweeps the offline directory after walking the
catalog, because the catalog is not a complete account of what is on disk — the
orphaned `.part` is exactly the file the button exists to reclaim. Callers
cancel outstanding work first, so nothing swept is being written to. Enqueue
gained a `StatFs` precheck against the request's own total with 256 MiB of
headroom, refusing as a recorded failure rather than an ENOSPC, and the work
constraint gained `setRequiresStorageNotLow`.

**Verified.** Compiles and passes `check`. The reclaim path needs a device with
a real partial download.

### B31 — download progress wrote to disk a dozen times a second

**Symptom.** Notification rate-limiting warnings and visible jank during a fast
download; a pause could silently revert.

**Cause.** The observer ran per 4 MiB block and each invocation did a
`setProgressAsync` (a WorkManager DB write), a `setForegroundAsync`, a
`createNotificationChannel` re-run, and a `commit()`-backed read-modify-write of
the retained record. At 50 MB/s that is roughly twelve of each per second. The
read-modify-write was not atomic against `pause()`: a write landing between the
two halves put `RUNNING` back over `PAUSED`, after which the worker's own
cancellation handler saw a status that was not `PAUSED` and recorded
`CANCELLED`. The pause was lost.

**Fix.** A report is made only when the percentage moves or a second has passed,
and the final one is never suppressed. The channel is created once per run.
`DownloadStateStore.update` does the read and the write under one process-wide
lock, and `pause`/`cancel` go through it too — which also stops a screen's stale
copy from clobbering the progress the worker has since written.

**Verified.** Compiles and passes `check`, host tests included. The lost-pause
race needs the concurrent-pause acceptance case.

### B14 — a failed episode was marked watched and the next one autoplayed

**Symptom.** An episode that failed to demux was recorded as watched, the player
advanced to the next one, and the foreground service tore down. No error was
shown anywhere.

**Cause.** `pstr_mpv.cpp` set `state_.ended` on `MPV_EVENT_END_FILE` without
reading `mpv_event_end_file.reason`, so `EOF`, `STOP`, `ERROR` and `REDIRECT`
were indistinguishable. Three consumers read the flag as "played to the end":
the watch-state write, the autoplay advance, and the service's teardown. The
desktop player has always modelled this — `pstr-player/src/player.rs:46-70`
carries `EndReason::{Eof,Failed,Stopped}` — so it was an Android-only
divergence.

**Fix.** `EndReason` now exists on both sides of the JNI boundary with the same
five values, filled from `mpv_event_end_file.reason` and carried through
`MpvPlaybackState.endReason`. The watched write and the autoplay advance test
for `Eof` alone; `Failed` puts a message in the player's error slot instead of
silently skipping the episode.

**Verified.** Compiles for both shipped ABIs; the behaviour needs the `playback`
acceptance case against a file mpv cannot demux, which does not exist yet.

### B15 — an episode transition could release the incoming episode's own stream

**Symptom.** Changing episodes intermittently killed the new episode
immediately, or skipped it outright.

**Cause.** `loadfile` is asynchronous. `Player::load` reset the state and bumped
the generation counter, then mpv stopped the outgoing file and emitted
`END_FILE(STOP)` — which the event thread applied to the *already bumped*
generation. The 250 ms poller could sample inside that window and produce
`MpvPlaybackState(ended=true, media=<new key>)`, a reading that passed every
generation guard in the code. The service then released the stream handle
Kotlin had already swapped to the new episode's, so every subsequent
`stream_read` returned `-1`. Separately, `NativeMpvHost.play` released the
outgoing handle the instant `nativeLoad` returned, while the outgoing demuxer
might still be reading it.

**Fix.** The native state carries a `loading` flag, set when `loadfile` is
issued and cleared by mpv's own `START_FILE`. Everything mpv reports in that
window describes the outgoing file, and is now discarded rather than attributed
to the incoming one — the end reason above all, but also position, duration and
the video dimensions. (`pause`, `volume` and `mute` belong to the core, carry
across a load, and are still applied.) Stream release moved out of Kotlin
entirely and into `stream_close`, which is the one moment defined to be after
mpv's last read.

**Verified.** Compiles for both ABIs. Needs the rapid-episode-switch acceptance
case.

### B17 — autoplay into the background crashed the app

**Symptom.** With background audio on, an episode ending while the activity was
stopped could crash with `ForegroundServiceStartNotAllowedException`.

**Cause.** The service retired itself — `stopForeground` plus `stopSelf` — the
moment a file ended, and `onPlaybackStarted` then called `startService` +
`startForeground` for the next one. Composition survives `onStop`, so autoplay
runs with the app backgrounded, and by then the app held no foreground state at
all: the API 31+ background-start restriction applied to precisely the case the
`backgroundAudio` setting exists to serve.

**Fix.** Teardown is deferred rather than immediate. `NativeMpvHost.advancing`
is raised by the player before it opens the next episode's stream and lowered
when `play` returns either way; the service waits it out before retiring, with a
30 s ceiling so a failed transition still ends. `startForeground` is not
re-entered when the service is already foregrounded, and the start pair is
wrapped so the exception is a lost notification rather than a crash.

**Verified.** Compiles. Needs an acceptance case that ends an episode with the
activity stopped.

### B18 — a Keystore failure panicked across the FFI and left no way back

**Symptom.** If the Keystore key was invalidated — lockscreen change, device
restore, alias loss — every share became permanently unopenable, and the user
saw a Java stack trace in a snackbar.

**Cause.** `BridgeError` did not implement `From<UnexpectedUniFFICallbackError>`,
so uniffi's generic converter routed to `handle_callback_unexpected_error`,
which panics unconditionally — on whichever thread was calling, including a
tokio worker in the middle of a download. `KeystoreSecretStore` threw raw Java
exceptions throughout, none of them a `BridgeError`. `DownloadObserver`'s
methods were not `Result`-typed at all, so a `setForegroundAsync` throwing
(routine on API 31+ when backgrounded) panicked mid-download. And there was no
recovery path: only Remove-and-re-add, which the same error blocks, because
removal deletes a secret the store can no longer read.

**Fix.** Four parts. `BridgeError` implements the conversion, so an unmapped
Kotlin exception is an ordinary error. `KeystoreSecretStore` wraps every
operation and leaves only `BridgeException`s, carrying the message and not the
stack trace. `DownloadObserver`'s two methods are fallible: a progress report
that cannot be delivered is logged and ignored, and an unanswerable cancellation
question is answered "no", because the `.part` file makes the next attempt a
resume either way. For recovery, `ShareStore::replace_secrets` re-supplies a
share's link while keeping its id — so the catalog rows and offline files keyed
by it survive — reachable from a "Re-enter link" action on each share. It
refuses a link for a different token. `KeystoreSecretStore.set` also replaces a
permanently invalidated key rather than failing against it, since nothing that
key sealed was readable anyway.

**Verified.** `cargo test -p pstr-core shares` covers both halves of
`replace_secrets`; the instrumentation tests for tampered, truncated and
key-destroyed payloads now assert `BridgeException` specifically. The
end-to-end recovery flow needs a device.

Not a defect, recorded so it is not re-litigated: the IV handling is correct — a
fresh random IV per `Cipher.init(ENCRYPT_MODE)`, prepended, 128-bit tag. The key
is still not auth-bound and `setUnlockedDeviceRequired(true)` is still not set,
which remains a deliberate-looking choice that has never been written down.

### B19 — one oversized poster crashed the app on every launch

**Symptom.** The library grid OOM-crashed, and kept crashing after a restart
until app data was cleared.

**Cause.** `RemoteArtwork.kt` bounded the *compressed* download at 12 MiB and
then decoded with no `inJustDecodeBounds` pre-pass and no `inSampleSize`. A
12 MiB PNG can be 12000×12000, which is a 576 MB `ARGB_8888` allocation. The
file was written to the cache *before* the decode, and that cache was never
pruned and had no size cap, so the poison persisted. There was no in-memory
cache either: `remember(url)` is per-composition, so scrolling a
`LazyVerticalGrid` re-read and re-decoded at full resolution on every item
re-entry.

**Fix.** A bounds pass settles an `inSampleSize` against a 1024 px longest edge
before anything is allocated. Bytes are written to the disk cache only once they
have decoded, so a hostile image is a missing poster rather than a permanent
crash. The directory is pruned oldest-first to 48 MiB after each new entry, and
a byte-sized `LruCache` in front of it — capped at an eighth of the heap, 4–32
MiB — means a scroll costs neither a read nor a decode.

**Verified.** Compiles and passes `check`. The crash-loop case needs a device
and a deliberately large image.

### B28 — closing the player leaked the Rust stream every time

**Symptom.** Ring-buffer and disk-cache handles accumulated for the life of the
process, once per episode change.

**Cause.** The player disposed with `scope.launch { session.closeEngine() }`
where `scope` was a `rememberCoroutineScope()`. `launch` dispatches through
`AndroidUiDispatcher` rather than running inline, and by the time it would have
run the composition had left and that scope was cancelled — so
`releaseStream` never executed. The session is `remember(key)`, so this fired on
every episode change as well as every exit.

**Fix.** `RustStreamSession.close` runs the release on a process-lifetime scope
of the session file's own, matching the lifetime of the engine it releases to.
The one moment this has to run is the one moment a composition scope is
guaranteed to be cancelled, so the scope cannot be the caller's.

**Verified.** Compiles and passes `check`.

### B32 — the media notification and lock-screen control were unusable

**Symptom.** The collapsed media notification showed no buttons, no title and no
art; the notification was rewritten four times a second.

**Cause.** `PlaybackService`'s `onStateChanged` callback rebuilt both
`PlaybackState` and `MediaMetadata` on every poll tick, unconditionally. The
metadata carried only `METADATA_KEY_DURATION` — no title, artist or art — and
`notification()` hardcoded `"proton-stream"` / `"Playing"` rather than the
episode. `Notification.Action.Builder` was passed a `null` icon while
`setShowActionsInCompactView(0, 1)` asked MediaStyle to render icons, so the
buttons were invisible. Neither the state actions nor the notification carried
skip-next/previous, so there was no episode control from the lock screen or a
Bluetooth remote, and `setSessionActivity` was unset.

**Fix.** The player publishes a `NowPlaying` — episode, show, detail, poster URL
and whether the playlist has an episode either side — through `NativeMpvHost`,
because the composition that knows those things is unreachable from a service.
`PlaybackService` collects it, fetches the poster through the same HTTPS-only,
disk-cached loader the library grid uses (so the shade costs no second
download), and builds `MediaMetadata` with title, artist, description and art.
The notification gains framework icons, previous/next actions gated on the
playlist position, `setSessionActivity`, a colorized MediaStyle on the app's
accent, and a dedicated monochrome status-bar glyph. Re-posting is gated on a
`NotificationShape` — the inputs the notification is actually built from — so a
poll tick that changes only the position no longer rewrites it.

**Verified.** Compiles and packages; `check` clean. The rendered result needs a
device.

### B40 — Picture-in-Picture was entered but never adapted to

**Symptom.** The full controls overlay, the skip button and the up-next card
drew inside the PiP window, which itself had no play/pause control. On a device
using gesture navigation it was usually never entered at all.

**Cause.** There was no `onPictureInPictureModeChanged` override anywhere.
`MainActivity` set no `setActions`, hardcoded `Rational(16, 9)` rather than
deriving the video's aspect, and called `setAutoEnterEnabled(true)` on params
that were passed to `enterPictureInPictureMode` but never registered via
`setPictureInPictureParams` — so auto-enter never armed, leaving only
`onUserLeaveHint`, which the swipe-home gesture does not send.

**Fix.** `mpv`'s `video-params/dw`/`dh` are observed and carried through
`nativeState` into `MpvPlaybackState`, giving the real aspect, clamped to the
range Android will accept rather than left to be refused outright on a 2.40:1
release. `MainActivity` folds those plus the paused flag into a
`PictureInPictureShape` and re-registers `setPictureInPictureParams` whenever it
changes, which is what arms auto-enter; the params carry previous/play-pause/next
`RemoteAction`s and a source-rect hint so the transition shrinks the video
already on screen. The remote actions are delivered by a runtime-registered,
non-exported `BroadcastReceiver` rather than a service intent, PiP being one of
the states in which a background service start is refused.
`onPictureInPictureModeChanged` drives a flag through `ProtonStreamApp` into
`PlayerScreen`, which then draws the picture and nothing else — no title bar, no
transport, no skip offer, no up-next card — and restores the controls on the way
out rather than leaving them mid-timeout.

**Verified.** Compiles and packages; `check` clean, and the
`PictureInPictureIssue` lint entry is gone from the baseline rather than
suppressed. The `picture-in-picture` acceptance case is now implementable — it
was `pending` precisely because the assertion would have had to assert this bug.

### B45 — the play/pause button did nothing, everywhere

**Symptom.** Once an episode was open, every transport that can pause it was
dead: the player's own button, the media notification's, the one in the
notification shade's media control, and the lock screen's. All four drew "Play"
over a file that was plainly playing, and pressing them changed nothing.

**Cause.** `Player::load` (`pstr_mpv.cpp`) resets `state_` to a fresh
`PlaybackState` before issuing `loadfile`, deliberately, so that the outgoing
file's clock is not read under the incoming file's name — and `PlaybackState`
defaults `paused` to `true`. But `pause` is a property of the mpv *core*, not of
the file: mpv carries it across `loadfile` unchanged and therefore never
re-publishes it. The observer that would have corrected the reset only fires on
a change that never comes, so `state_.paused` stayed `true` for the whole
episode.

Everything downstream believed it. The transport drew Play, and
`host.setPaused(!state.paused)` therefore sent *unpause* to a core that had
never paused — a no-op, and the icon did not move afterwards either.
`PlaybackService` published `STATE_PAUSED` for the same reason, so the shade's
media control offered Play and its `onPlay` took the same no-op path. Only
volume and mute escaped, being the two fields `load` already carried across.

**Fix.** `load` preserves `paused` alongside `volume` and `muted`, with the
reason recorded where the reset is. `Player::pause` also writes the flag through
to `state_` rather than waiting for the observer, so a tap moves the transport on
the frame it happens rather than on the next poll a quarter second later, and
the UI is right even if the property event is ever lost. `NativeMpvHost.setPaused`
mirrors that write into `MpvPlaybackState` and notifies the service, so the
notification flips with the button.

**Verified.** Compiles and packages for both shipped ABIs; `check` passes with
no new lint findings. The behaviour itself needs the `playback` acceptance case
on a device.


### B43 — the acceptance harness could not run against the shipping build

**Symptom.** The first run on real hardware failed `native-libraries` on an APK
that plainly contained them, and could not have run against a release APK at all.

**Cause.** Four defects, none of which a device-free run could expose. The
package name and the activity class were treated as sharing a prefix, so `launch`
built `<package>/.MainActivity` — the leading dot expands against the *package*
argument, which on a debug build names `io.narl.protonstream.debug.MainActivity`,
a class that does not exist. `PACKAGE` was hardcoded to the debug applicationId,
so pointing the suite at a release APK found nothing installed. The
`native-libraries` case listed the on-device `lib/` directory, which
`useLegacyPackaging = false` leaves legitimately empty because the libraries are
mapped straight out of the APK. And `assert_no_crash` filtered logcat on
`PACKAGE.split(".")[0]` — the string `"io"` — then failed on any `FATAL` line,
so `offline-launch` failed on the Google Play services crash that its own
airplane-mode toggle provokes.

**Fix.** The activity is named by its fully-qualified class; `--release`,
`--package` and `--apk` resolve the APK and applicationId together;
`native-libraries` pulls the installed `base.apk` and reads its `lib/<abi>/`
entries, which also works under `--no-install`; crash detection is scoped to the
app's own process by the `Process:` line the runtime prints under a
`FATAL EXCEPTION` and by `>>> <package> <<<` in a native tombstone. Two further
fixes came out of the same run: the `instrumentation` case shelled `gradlew`
without `JAVA_HOME` or `ANDROID_TEST_BUILD_TYPE`, so it inherited a JDK 26 and
asked for a task that only exists when `testBuildType` is `release`; and cases
that start the app now wake and unlock the screen first, because a dark screen has
no resumed activity and the failure looks like a crashed app.

**Verified.** Full matrix green on a Pixel 6 (API 37) against the signed release
APK; see [B42](#b42--nothing-verifies-any-of-the-above-on-a-device).

### B44 — R8 broke the app under instrumentation in five places

**Symptom.** `connectedReleaseAndroidTest` died in `newApplication` before a
single test ran. Fixing that produced another crash, five times over.

**Cause.** The instrumentation APK is minified in its own R8 run that consumes
the app's mapping file and links against the app's classes at runtime. That
arrangement breaks in two ways: the mapping carries renames but not R8's
parameter-permutation optimisation, and a class only the test APK reaches is
unreachable from the app's call graph, so R8 removes it from the app while the
test APK does not bundle it. In order:

1. R8 rewrote `Intrinsics.checkNotNullParameter(Object, String)` to take
   `(String, Object)`. Every Kotlin class in both APKs calls it on entry —
   `NoSuchMethodError: No static method f(Ljava/lang/Object;Ljava/lang/String;)V
   in class Lo6/k;`.
2. `androidx.tracing.Trace`, which `AndroidJUnitRunner.onCreate` uses and nothing
   in the app reaches, was dropped.
3. `androidx.work.WorkManager` is absent from `mapping.txt` entirely rather than
   renamed: the app only touches the implementation, so R8 vertically merged the
   abstract class into `WorkManagerImpl` and JUnit's field scan could not
   construct the test class.
4. `DownloadStateStore.clear()` has exactly one call site (`AppViewModel`), so R8
   inlined it and dropped the method — invisible to the app, fatal to a caller
   that resolves it by name.
5. The same for `SettingsStore.setWifiOnly`, and then for the synthesised
   `DownloadCoordinatorKt` facade that holds the package's top-level functions.

**Fix.** `android/app/proguard-instrumentation.pro`, applied to the release
build, with each keep annotated by the crash it repairs. Only the last group is
app code, and the file states the trade-off plainly: those classes are no longer
optimised in the shipping build, so the suite no longer proves R8 handles them —
a narrow loss, since the surfaces R8 has actually broken here are the FFI ones,
and those are kept for functional reasons in `proguard-rules.pro` and stay under
test. The APK grew from 102.6 MB to 105.1 MB, roughly 2.4%, on a package that is
almost entirely native libraries.

**Verified.** 20/20 instrumentation tests pass on the signed minified release
APK, including the whole UniFFI boundary and the Keystore crypto suite — which is
the first direct evidence that R8 does not break the FFI.

### B13 — the player's controls ran off the bottom of the window

**Symptom.** On the player page the transport row — previous, ±seconds, play,
volume, the pickers — sat flush against the bottom edge of the window and was
clipped by it. The floating **Skip opening** button sat at the same height as
the seek bar rather than above the controls, four pixels out of line with them.

**Cause.** `CHROME_HEIGHT` was a constant 132 px. The controls are two rows of
themed buttons whose height follows the theme and the platform's text scaling,
so the constant was a guess — and a guess that is low does not clip in egui, it
draws past the rectangle and off the window. The skip button then used its own
22 px inset where the controls used 26.

**Fix.** `ui::player::chrome` returns the height it actually measured
(`ui.min_rect().height()` plus padding), the app carries it in `Overlay`, and the
next frame uses it for the scrim and for floating anything above the controls.
The content is anchored to the bottom of the window rather than laid out from
the top of the strip, so the last row ends a fixed distance above the edge
whatever it turns out to contain. One frame of lag, and only while resizing.
`floating` puts both the skip button and the up-next card at the controls' own
`CHROME_PAD_X`.

### B12 — the skip button believed a chapter's name

**Symptom.** Episode one of Oshi no Ko opens with a chapter called `Intro` that
is eleven minutes of the story. The player offered **Skip opening** over it.

**Cause.** `ChapterRole::of` read the first word of the name and nothing else,
mapping `intro` to `Opening` unconditionally. Nothing looked at how long the
chapter was or at what else the file contained.

**Fix.** Two passes — `Claim` from the name, `roles` from the name *and* the
file. A name that could go either way (`Intro`, `Credits`, `Cast`) is resolved
by length and position: an opening has to be theme-length, near the front, and
the only candidate in the file. Both ambiguities fail towards content. The same
pass is what `credits_start` reads, so the end-of-episode countdown does not
assume the last chapter is the ending either — see the Part C case in
`docs/DEVELOPMENT.md`.

**Verified.** `a_long_intro_is_the_episode_and_not_an_opening`,
`a_theme_length_intro_near_the_front_is_an_opening`,
`content_after_the_ending_keeps_the_ending_out_of_the_tail`.

### B11 — seasons past the first had no episode names, and were one query from having the wrong ones

**Symptom.** Oshi no Ko seasons two and three showed filenames where season one
showed episode names. Season one only "worked" because *its* filenames carry
them (`S01E01-Mother and Children.mkv`); `episode_metadata` was empty for the
whole library.

**Cause.** Two, stacked. A title is looked up once, and on AniList that match is
the *first season's* entry — a sequel is a separate id numbering from one — so
nothing was ever asked about seasons two and three. And `EpisodeGuide::get` fell
back from `(Some(2), 1)` to the provider's absolute `(None, 1)`, so the moment
season one's episodes were fetched, every later season would have been captioned
with season one's names. A wrong caption does not read as a bug; it reads as the
library being wrong.

**Fix.** `Provider::seasons_are_separate_entries` distinguishes the two
providers. For AniList, `MetadataService::season_episodes` searches each season
past the first by the name the provider uses (`<title> 2nd Season`, then
`<title> Season 2`) and tags the result with that season; TMDB already returns
seasons numbered and is left alone. The absolute fallback now stops at season
one.

**Verified.** `an_absolutely_numbered_answer_never_reaches_a_later_season`,
`a_season_is_searched_for_the_way_the_provider_names_it`. Against the live API,
`[Oshi no Ko] 2nd Season` is id 166531 with 13 episodes and `3rd Season` is
182587 with 11 — which is exactly what the share holds.

**Still missing for that show, and not a bug here.** AniList's episode titles
come from `streamingEpisodes`, and all three Oshi no Ko entries have none at
all. Those rows keep their filenames until the viewer switches to TMDB.

### B10 — every episode of season three was called "3rd Season"

**Symptom.** Under `Oshi no Ko`, all eleven rows of season three read
`3rd Season` instead of an episode name.

**Cause.** `naming::parse` knew `S03` and `Season 03` but not `3rd Season`, so
`[DB]Oshi no Ko 3rd Season_-_07_….mkv` parsed as a title of `Oshi no Ko 3rd
Season`. `catalog::build_rows` then took the difference between that and the
folder's `Oshi no Ko` to be the *episode's* own name — which is what it is for,
and here it was a season marker.

**Fix.** `find_worded_season` reads both orders (`3rd Season`, `Season 3`) and
ends the title there, so the season comes out of the filename as a season.
`build_rows` additionally asks `naming::is_season_marker` before keeping any
leftover as an episode name, and folds it into the season instead. The dash is
deliberately not a separator between `Season` and its number: `Show 2nd Season -
04` puts the episode there, and reading across it made episode four season four.

**Verified.** `underscore_separated_numbering_is_read`,
`a_worded_season_is_read_as_a_season`,
`an_ordinal_that_is_part_of_the_title_is_not_a_season`,
`a_season_stated_in_the_filename_does_not_become_the_episodes_name`.

### B9 — a series split into one poster per season on the shelf

**Symptom.** The library wall showed `Game of Thrones - Season 1` … `Season 8`
as eight separate titles, each with its own poster and its own resume point.

**Cause.** `naming::classify_folder` recognised a season folder only when the
whole name was one (`Season 01`, `S03`). A folder named `<Series> - Season 1`
therefore classified as a title, and the season number stayed in the title
string, so the grouping key differed per season.

**Fix.** `naming::trailing_season` splits a `… Season N` / `… SNN` tail off a
folder name, and `catalog::ancestry` takes the season from a folder that names
both. The keyword is required — `Evangelion 3` and `Alien 3` keep their number,
and `Blade Runner 2049` still reads as a year.

**Verified.** `a_folder_naming_series_and_season_yields_both`,
`a_title_ending_in_a_number_keeps_it`; on the real share, 30 titles became 23,
with Game of Thrones one poster of 73 episodes.

### B5 — a multi-byte character in a filename panicked the parser

**Symptom.** The crawl aborted on
`[Anime Time] Neon Genesis Evangelion - Death (True)²-007.mkv` with
`start byte index 34 is not a char boundary; it is inside '²'`.

**Cause.** Year detection scanned bytes and sliced on byte offsets, so a token
boundary could land inside a multi-byte character.

**Fix.** `bare_year` walks `char_indices`; `bracketed_year` verifies the four
bytes are ASCII digits *before* slicing.

**Verified.** `a_multi_byte_character_does_not_break_parsing`.

### B4 — files were catalogued with no title

**Symptom.** Every episode in the catalog had an empty title: a real library is
laid out `Sousou no Frieren/Season 01/S01E01.mkv`, and `S01E01.mkv` states no
series at all.

**Cause.** Rows were built from the filename alone.

**Fix.** `catalog::build_rows` walks the folder path: the folder names the
series, the filename names the numbering, grouping folders (`Movies`, `OVA`,
`Specials`) are stepped over, and a `Season NN` folder supplies a season the
filename omitted. Non-video files (`.nfo`, `.jpg`) are dropped.

**Verified.** Seven `build_rows` tests, and a real 354-node share that went from
0 usable titles to 22 correct ones.

### B3 — several naming conventions lost their episode numbers

**Symptom.** 35 of 310 files in a real share had no episode number.

**Cause.** Four gaps: a bare `E11.mkv`; a bare `11.mkv`; `_-_01_` underscore
separators; and — the subtle one — `Oshi no Ko S2 - 04`, where matching the
season *returned*, so the differently-stated episode was never looked for.

**Fix.** `find_episode_marker`, `find_bare_number`, underscore separators in
`find_dash_numbering`, and `find_numbering` continuing past a season-only match.

**Verified.** Down to 11 unnumbered files, all of which are films or OP/ED
extras that genuinely have no episode number.

### B1 — a public-link download silently truncated past 200 MiB

**Symptom.** `download_file` over a public link returned a file cut off at
exactly 50 blocks, with no error.

**Cause.** The visitor path issued a single un-paged
`GET …/revisions/{rid}` and used whatever `Blocks` came back. The endpoint pages
at 50.

**Fix.** `proton-sdk-rs` 0.3.3: the visitor path now uses the same paged
`list_blocks` as the authenticated one, and fetches blocks concurrently.

**Verified.** `a_visitor_can_seek_within_a_shared_multi_block_file` reads back a
9 MiB file whole and in ranges.

### B2 — a catalog crawl re-derived the same keys per child

**Symptom.** Listing a folder of several hundred episodes over a public link took
minutes.

**Cause.** `resolve_parent_key` walked and re-*unlocked* the whole ancestor chain
for every child. Each unlock is an S2K derivation — tens of milliseconds — so 500
siblings cost 500 redundant unlocks of the same parent key. `enumerate_nodes`
also never chunked its link ids, exceeding the server's 150-id batch limit.

**Fix.** `proton-sdk-rs` 0.3.3: an LRU node-key cache with an iterative
ancestor walk (up to the nearest cached key, then unlock back down caching each)
behind `SingleFlight`; `enumerate_nodes` chunked at 150 and fanned out.

### B6 — read-ahead starved the seeks it was supposed to smooth

**Symptom.** Dragging to a new position took ~2.9 s with the block layer's
read-ahead enabled, versus ~0.57 s with it turned off. Sustained throughput was
also *lower* with read-ahead than without: 5.0 MiB/s against 7.2.

**Cause.** Prefetches queued from the position the viewer had just left kept
running. The block they were fetching was exactly the bandwidth the seek needed,
and a 4 MiB block over this link takes most of a second, so the seek waited
behind up to six of them.

**Fix.** `pstr-stream::stream` tracks where the previous read ended and treats a
read landing more than one block away as a seek. A seek cancels every outstanding
prefetch *before* it fetches anything. Aborting mid-fetch throws those bytes
away, which is the intended trade — they were speculative, and the request slot
they held is what the seek is waiting for. Read-ahead resumes wherever the seek
settles.

**Verified.** Worst-case cold seek back to 715 ms with read-ahead on.
`a_seek_cancels_the_read_ahead_it_invalidates` and
`sequential_playback_never_cancels_its_own_read_ahead` pin both directions, and
`a_block_whose_prefetch_was_cancelled_can_still_be_read` pins that an aborted
prefetch releases its single-flight entry rather than leaving the block
permanently unfetchable.

### B7 — a read-ahead window deeper than the ring evicted its own blocks

**Symptom.** Raising read-ahead from 12 to 32 blocks *reduced* sustained
throughput, 8.7 MiB/s to 7.8, and the ring started reporting evictions.

**Cause.** The default 128 MiB ring holds 32 blocks of 4 MiB. A 32-block
read-ahead window is the entire ring, so blocks at the front of the window were
evicted before the player reached them and had to be fetched a second time.

**Fix.** `clamp_readahead` caps the window at half the ring's capacity in blocks
— half rather than all, because the ring also has to hold what the player has
just passed, so a scrub a few seconds backwards does not refetch. The default
depth is 12, chosen because it never evicted and wasted a third less bandwidth on
cancelled prefetches than 16 did, for indistinguishable throughput.

**Verified.** `read_ahead_is_capped_at_half_the_rings_capacity` and
`a_tiny_ring_still_reads_one_block_ahead`.

### B8 — the block layer's read-ahead made playback worse once mpv was in front of it

**Symptom.** With `pstr-stream`'s tuned default of 12 blocks, mpv took 5.5 s to
show a first frame and 3.6 s to resume after a mid-file seek. Turning the block
layer's read-ahead **off entirely** cut both to 2.5 s and 1.5 s, and fetched a
third as much data for the same playback.

**Cause.** [B6](#b6--read-ahead-starved-the-seeks-it-was-supposed-to-smooth) and
[B7](#b7--a-read-ahead-window-deeper-than-the-ring-evicted-its-own-blocks) were
measured against `pstr bench`, a reader with no read-ahead of its own — for
*that* reader, 12 blocks is right. mpv is not that reader. Its demuxer cache
already runs `demuxer-readahead-secs` ahead of the picture, sequentially and
eagerly, so a second speculative layer underneath adds no buffer at all. It only
competes for bandwidth with the reads mpv is blocked on right now, and it does so
worst exactly when the viewer is waiting: at startup and just after a seek. The
degradation is monotonic in depth.

| blocks ahead | first frame | seek resumed | blocks fetched |
|---:|---:|---:|---:|
| **0** | **2.5 s** | **1479 ms** | **12** |
| 6 | 3.7 s | 2225 ms | 21 |
| 12 | 5.5 s | 3581 ms | 31 |
| 24 | 7.7 s | 6122 ms | 42 |

**Fix.** `pstr_player::READAHEAD_BLOCKS` is 0, and it is what `pstr play` and the
app pass to `StreamConfig`. `pstr_stream::DEFAULT_READAHEAD_BLOCKS` stays 12 for
consumers that genuinely have no read-ahead — the benchmark, and anything that
reads a revision without a demuxer. mpv's buffer is also expressed in *seconds*
rather than bytes (`PlayerConfig::readahead_seconds`, 30 s), because a byte
budget buys two minutes of buffer on a 4 Mbit/s episode and eight seconds of it
on a 4K remux, which is backwards from what either wants; the byte figure is kept
only as a ceiling, sized under the ring so B7 cannot recur one layer up.

**Verified.** Sustained playback holds realtime at depth 0 (178 s of media in
180 s of wall clock, the deficit being the 2.4 s to first frame), with no ring
evictions, against the same 761 MiB episode over a real public link.
