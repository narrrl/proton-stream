# Development

## Where things stand

| Layer | State |
|---|---|
| SDK streaming surface (`../proton-sdk-rs` 0.4.0, from crates.io) | **done, live-verified** — `open_revision` / `download_range` / thumbnails / `refresh_session` on the visitor path; node-key cache; chunked `enumerate_nodes`; paged `download_file_to` |
| `pstr-core` — shares, config, catalog, naming, library grouping | **done** — 106 offline tests |
| `pstr-cli` — add / shares / remove / crawl / list / metadata / play / bench | **done** — verified against a real 354-node share: 310 playable files, 22 titles, correct seasons and episodes |
| `pstr-stream` — block cache, read-ahead | **done, live-verified** — 68 offline tests; benchmarked against a real 761 MiB episode (see `docs/TESTING.md`) |
| `pstr-player` — libmpv `stream_cb` + render API | **done, live-verified** — 33 offline tests; plays a real 1080p HEVC episode over a public link, seeks, resumes from the disk cache, and renders into the app's GL context |
| `pstr-app` — egui UI, poster grid, embedded video | **done, offline-tested** — 15 offline tests; library wall, title pages, player page with overlay controls, themes, share management |
| Metadata providers (AniList / TMDB) | **done** — 32 offline tests; posters, synopses, per-episode titles, hand-pinned matches. Opt-in, off by default |
| Packaging | **done, locally verified** — `scripts/build.sh` (tarball, `.deb`, `.rpm`, `.app`, `.dmg`) and `scripts/build.ps1` (portable `.zip`, WiX `.msi`), plus `packaging/PKGBUILD`. See `packaging/README.md`. The macOS half is written but has not been run on a Mac |

## Build order from here

1. **Sidecar subtitle files** — see the "not done" note at the end of this
   section. The work is in `pstr-core`: crawl them, and pair them to a video by
   name.
2. Packaging is in place (`packaging/README.md`); what is left there is running
   the macOS half on a real host, and a release workflow that drives every
   platform.

Done: **`pstr-player` step two** — `mpv_render_context_create` with
`MPV_RENDER_API_TYPE_OPENGL` against eframe's glow context
(`pstr-player/src/render.rs`), painted from a framebuffer this app owns
(`pstr-app/src/video.rs`) so the picture can sit in a rectangle rather than
filling the window. `VideoOutput::Embedded` is what the app asks for; the
standalone-window mode the CLI uses is unchanged, and nothing about it reaches
the streaming path.

Done: `pstr-player` step one — `mpv_stream_cb_add_ro`, mpv in its own window.
`pstr play --file <text>` plays a catalogued episode over a public link.

Done: `pstr-app` step one — `cargo run -p pstr-app` opens a window with the
library wall, a title page per series, a transport bar and share management. It
plays through the same `Player` the CLI uses — at that point still in mpv's own
window; the app's transport drives it from a command channel. Two pieces of that are
worth knowing before touching it:

- **Drawing never mutates.** Pages are handed `&`-state and push `Action`s onto
  a list the app applies after the frame. That is what lets a card click change
  the page it was drawn on.
- **The UI thread never opens a share, reads the catalog or decodes a JPEG.**
  All of it goes through `pstr_app::engine::Engine`, which spawns onto the tokio
  runtime and answers with an `Event` — plus a `ctx.request_repaint()`, without
  which the event sits in the channel until the mouse moves.

Done: player controls beyond play/pause/seek — volume with mute, an audio-track
picker and a subtitle picker, in the transport bar (so on the player page and on
the bar under the library alike), plus `M` and `↑`/`↓` on the player page. All
four preferences persist to `playback.json` (`pstr_core::prefs`), and a track
chosen by hand sets `alang`/`slang` for every file loaded afterwards — the point
being that picking Japanese audio once picks it for the rest of the season.
A dragged volume slider writes to disk on release, not per frame.

Done: the player page's overlay — a full-width seek bar with the file's chapters
marked on it and the current chapter named beside the clock, a transport cluster
(previous / −10s / play / +30s / next), and volume plus the audio, subtitle and
chapter pickers to the right. Chapters come from mpv's `chapter-list`
(`pstr-player/src/chapters.rs`); a chapter whose name reads as an opening,
ending or preview also puts a **skip** button in the bottom-right corner, which
is drawn whether or not the rest of the controls are up — an opening lasts
ninety seconds and a button you have to wake with the mouse is one nobody uses.
The strip under the library is the same controls at a lower density plus a
**Back to video** button, which is what makes leaving the player page with Esc
reversible.

Done: **next / previous episode and autoplay.** The neighbours come from
`Title::following`/`preceding` in `pstr-core` — display order, so it walks off
the end of a season into the next one — and autoplay fires only on a clean end
of file, never on a failure. `P`/`N` on the player page, and the page stays put
while the next file opens rather than dropping back to the library.

Done: **what a chapter is, and the "up next" countdown.** Chapter names are read
in two passes (`pstr-player/src/chapters.rs`). The first reads the name alone —
`OP`, `ED`, `Next Episode`, and the Japanese forms — and yields a *claim*; the
second (`roles`) settles the claims that a name cannot settle, using the rest of
the file. `Intro` is the opening only when the file has no chapter that says so
outright, the chapter is the length of a theme song and it is near the front:
episode one of Oshi no Ko opens with eleven minutes called `Intro` that are the
story. `Credits` / `Cast` are the ending at about that length or in the last
quarter. Both ambiguities fail *towards content*, because not offering a skip
costs a button and skipping the story costs the story.

`credits_start` then takes the run of chapters that **ends** the file — walking
back from the last one and stopping at the first that is content. That is the
part worth stating: the ending is not the tail if something follows it.
Fullmetal Alchemist Brotherhood episode 46 has a Part C after the ED and the
preview after that, so only the preview is skippable there, while the ordinary
`… ED · Preview` gives up both. Reaching that run starts a ten-second **Up next**
card (`App::tick_up_next`, `ui::player::up_next_card`) with *Play now* and
*Watch till the end*; the latter holds for the rest of the file. It needs
chapters, autoplay on, a next episode, and playback actually running — a paused
episode is one somebody walked away from.

Done: **per-episode metadata.** `Provider::episodes` fetches an episode list for
a title that already matched (AniList `streamingEpisodes`, which is titles and
thumbnails only and no synopsis; TMDB `/tv/{id}` plus every season appended onto
one more request), stored in `episode_metadata` — schema **v4**. The reconciling
between a release's numbering and a provider's lives in one tested place,
`EpisodeGuide::get`: the season and number the filename states, then the
provider's absolute numbering, then season one for a file that states no season
at all. That last fallback is what makes `#057` line up with TMDB filing a
64-episode anime under one season. Episode names show on the title page's rows,
in the player's title strip and in the "next:" line autoplay prints.

**A season is a separate lookup on AniList.** `Provider::seasons_are_separate_entries`
is what splits the two providers: TMDB files every season under one show and
returns them numbered, while AniList makes each sequel its own entry numbering
from one (`[Oshi no Ko] 2nd Season`, id 166531, is not `[Oshi no Ko]`, id
150672). So for AniList each season past the first is searched for by name —
`<title> 2nd Season`, then `<title> Season 2` — and its episodes are stored
tagged with that season. The absolute fallback in `EpisodeGuide::get` stops at
season one for the same reason: without that it answered `S02E01` with season
one's episode one, which is not a missing caption but a wrong one.

Done: **matching a title by hand.** `MATCH_FLOOR` is set where a wrong poster
costs more than none, and the price of that is a few titles it refuses to decide
about — `Fate/stay night [Heaven's Feel]` is the standing example, three films in
one folder against a provider that files each of them as its own entry, so there
is no single answer for the scorer to find. *Change match* on the title page opens
`ui::matcher`: the provider's own answers for whatever is typed, unscored and in
its own order (`MetadataService::search`), and a click pins one. Nothing is
ranked there on purpose — the floor exists to stop the *scorer* guessing, and a
person reading eight entries with their posters is not guessing.

A chosen row is stored `manual` (schema **v5**), which means two things:
`MetadataRecord::is_fresh` never expires it, and `run_match` skips it even when
forced — "match again" must not undo the one title someone fixed by hand. The
way back is *Clear*, which drops the row entirely (`Catalog::forget_metadata`)
rather than storing a miss, so the next run treats the title as one it has never
asked about. Switching provider still clears everything, hand-picked rows
included: a `remote_id` means nothing to the other provider.

Worth knowing before chasing a missing name: AniList's episode titles come from
`streamingEpisodes`, which is what streaming sites published, and for some shows
it is simply **empty** — all three Oshi no Ko entries have none, while Fullmetal
Alchemist Brotherhood has 64. There is nothing to fix on our side for those;
TMDB is the provider that has them.

Note the glyph trap: egui's bundled fonts cover the transport icons
(`⏮ ⏭ ⏸ ▶ 🔊 🔇 ⛶`) and *not* `← 💬 ▣`, which draw as an empty box. Anything new
on a button wants checking on screen before it ships.

Not done: **sidecar subtitle files**. A `.srt`/`.ass` sitting beside the video in
the share is invisible to all of this — the crawl stores video files only, so
there is nothing to offer in the menu. The player side is nearly free once the
catalog has them (`sub-add pstr://<token>` through the same registry, since mpv
reads an external subtitle through the same protocol); the work is in
`pstr-core`: crawl them, and pair them to a video by name.

## Design decisions already taken

- **Fat client, no server.** Each viewer's app talks to Proton directly.
- **Public links only**, but several, merged into one catalog. No account login.
- **Embedded libmpv**, not a bundled decoder stack. HEVC 10-bit, HDR and ASS
  subtitles with hardware decode, for free.
- **No transcoding.** mpv demuxes and decodes natively; there is no remux or
  re-encode step and no ffmpeg dependency of our own.
- **`stream_cb`, not a loopback HTTP server.** No port to open, no local attack
  surface, no plaintext leaving the process.
- **Metadata enrichment is opt-in and off by default**, with the privacy cost
  stated in the UI: enabling it sends your library's titles to a third party.
  With it off, posters fall back to Proton thumbnails.
- **WASM is out.** Not a small port and not worth reopening: the SDK pulls tokio
  with `rt-multi-thread` + `fs`, has four `spawn_blocking` sites, resolves two
  `getrandom` versions with no wasm backend, and drags `aws-lc-rs`/`ring` through
  rustls. Separately a browser cannot reach the Proton API at all (CORS) without
  a proxy server, which the fat-client decision rules out.

## Traps

- **Block sizes are not uniform.** Always `RevisionReader::block_sizes()`; never
  a hardcoded 4 MiB. A padded vector shifts every later block's start and serves
  full-length reads of the wrong bytes, silently.
- **Cache validity is the revision id**, not `(mtime, size)`. It advances if and
  only if a new revision was sealed.
- **Read-ahead must yield to seeks, and must not outrun the ring.** Both were
  measured, both cost more than they bought when got wrong: prefetches left
  running from the position the viewer left took worst-case seek latency from
  572 ms to 2895 ms, and a window deeper than half the ring evicts its own blocks
  before the player reaches them. `pstr-stream::stream` cancels on seek and
  clamps the depth; the numbers are in its module docs.
- **Read-ahead depth is a property of the *consumer*, not of the block layer.**
  mpv brings its own, so under mpv the block layer's read-ahead should be **0**:
  at depth 12 the first frame took 5.5 s instead of 2.5 s and a seek 3.6 s
  instead of 1.5 s. `pstr_player::READAHEAD_BLOCKS` is what a player-driven
  caller passes; `pstr_stream::DEFAULT_READAHEAD_BLOCKS` is for readers that have
  none of their own. Do not "unify" them. See `docs/BUGS.md` B8.
- **mpv's buffer is in seconds, not bytes.** A byte budget means two minutes of
  buffer on a 4 Mbit/s episode and eight seconds on a 4K remux. The byte figure
  survives only as a ceiling, and it must stay under the ring or B7 recurs a
  layer up.
- **The anonymous session expires mid-film.** The SDK renews it in place and
  readers survive; the app should still be able to reopen and resume at the same
  byte offset as a belt-and-braces fallback.
- **A share filled by the FUSE client has no thumbnails at all.** Proton renders
  a thumbnail at upload time and the SDK does not generate one, so every
  `download_thumbnail` on this library answers `Ok(None)` and every tile falls
  back to initials. Nothing is broken — it is why metadata providers matter more
  than they looked like they did, and why an empty `thumbs/` cache directory is
  not evidence the thumbnail path is wrong.
- **Negative caching matters.** Without it, every grid render pays a round-trip
  per unmatched title. `proton-drive-linux`'s photo grid learned this the hard
  way. `ThumbnailCache` remembers both the textures it has *and* the files that
  turned out to have no thumbnail.
- **A dragged seek bar must seek on release, not per frame.** Every seek cancels
  outstanding read-ahead and refetches; a bar that seeks while dragging spends
  the whole drag throwing away blocks it just paid for. `ui::transport` acts on
  `clicked() || drag_stopped()`.
- **egui 0.35 moved the panels.** `eframe::App` is `fn ui(&mut self, ui, frame)`,
  not `update(ctx, frame)`; panels are `egui::Panel::top(id).show(ui, …)` rather
  than `TopBottomPanel::top(id).show(ctx, …)`, and `Context::style_mut` is now
  `all_styles_mut`. Older egui snippets will not compile as written.
