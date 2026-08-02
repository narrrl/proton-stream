# Bugs

The issue ledger. Open items first, then a log of what was fixed and where.

Format: `Bn — one-line summary`, then symptom, cause, fix, and how it was
verified. Reference entries from PRs.

## Open

_None yet — the app is young enough that everything known is in
`docs/DEVELOPMENT.md` as unbuilt rather than broken._

## Fixed

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
