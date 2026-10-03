# Metadata

How a library of filenames becomes posters, synopses and episode names, what
was wrong with the first version of it, and where it is going. The privacy
stance from `pstr_core::metadata` holds throughout: nothing here is requested
until the viewer turns enrichment on, and every new source named below sits
behind that same switch.

## What a real library showed

Audited against a 42-title, 758-file anime library matched on AniList
(schema v8, October 2026). The match itself — title to AniList entry — was
good. What hung off it was not:

1. **Later seasons captioned with season one's names.** Attack on Titan
   seasons 2–4 and Jigokuraku season 2 all stored season one's episode list.
   The per-season lookup searched for `Attack on Titan 2nd Season`, and the
   similarity scorer, seeing the base entry's alias inside the query, picked the
   base entry again. That is the failure `EpisodeGuide::get` already warned
   about: not a missing caption but a wrong one.
2. **`streamingEpisodes` is not an episode list.** It is what a streaming site
   published, partial and numbered however that site numbered it. Dan Da Dan
   had 5 rows numbered 13–17, Kaiju No. 8 had 4–15, The Apothecary Diaries had
   25–48 under two seasons at once, Bleach had 20 rows called `Untitled`, Akira
   had a trailer. No row anywhere had a synopsis — AniList keeps none.
3. **A release's season is not an AniList entry.** Bleach `S17` is the
   Thousand-Year Blood War, four separate AniList entries. Mushoku Tensei `S01`
   is two (11 + 12 episodes). Attack on Titan's season folders hold
   absolutely-numbered files — `S2` is episodes 26–37. Mushishi's `S03` is
   *Zoku Shou 2*, which TVDB files as the second half of season two.
4. **Answers outlived their match.** `jujutsu kaisen 0 movie` kept the TV
   series' 24 episode names after it was re-pinned to the film: an empty episode
   list was never written, so the old one stayed.
5. **Desktop art was the wrong shape.** The 16:9 tile and the library's hero
   prefer `backdrop_url`, which on AniList is `bannerImage` — a 1900×400 strip,
   4.75:1, cached at 640×135. Cropped to 16:9 and stretched, that is a blurred
   sliver from the middle of a banner. Android looks right because it draws the
   2:3 cover.
6. **"Newest" sorts on nothing.** It reads the year from the filename, which 2
   of 42 titles state.

## Sources

| Source | Key | What it is good for |
|---|---|---|
| **AniList** GraphQL | none | Identity. Search, every alias, cover art, relations (`SEQUEL`, `PREQUEL`, `SIDE_STORY`…), studios, staff, tags, format, season, popularity, next airing episode, the MAL id. |
| **ani.zip** `api.ani.zip/mappings?anilist_id=` | none | Everything AniList lacks, per AniList id: TVDB season, episode and absolute numbers; episode titles in several languages; TVDB synopses; 16:9 stills; air dates; runtime. Series art: TVDB **fanart at 1920×1080**, poster, banner, logo. Cross-ids for MAL, AniDB, TVDB, TMDB, Kitsu, IMDb. A community service — cache what it says. |
| **TMDB** | free key | Film backdrops and logos; live-action TV. Already implemented. |
| **TVmaze** | none | Live-action TV episodes, keyless. |
| **AniSkip** | none | Opening and ending timestamps by MAL id and episode. |
| **Jikan** (MAL) | none | Filler and recap flags. Timed out when tried; optional. |
| **manami-project/anime-offline-database** | none | Every anime with its synonyms, ids and relations, as one 6 MB `.zst`. Matching against it locally sends no title anywhere. |
| AniDB, TVDB directly | registration / paid | Covered by ani.zip, which republishes their data. |

The fact the whole design turns on: **release groups number files the way TVDB
does.** `Bleach - S17E01` is TVDB season 17; Attack on Titan's 26–96 are TVDB
absolute numbers. ani.zip gives both for every AniList entry, so the join
between a filename and an episode is a lookup, not a guess.

## Phase 1 — correct data (done)

**Seasons by relation, not by name.** `AniList::enrich` starts at the matched
entry and follows its `SEQUEL` edges — TV formats first, a special or film only
when it is the sole link onward (Mushishi's chain runs through an OVA). Each
entry's episodes come from ani.zip. The walk stops as soon as every numbered file
in the title has an answer, at the end of the chain, or after
`MAX_CHAIN_ENTRIES`, so a one-season show costs one ani.zip request and no
relations query at all.

**One episode, three numberings.** `EpisodeMetadata` carries TVDB's
`(season, number)`, TVDB's `absolute` number, and its `(entry_season,
entry_number)` — the position of its entry among the franchise's TV entries,
and its number within that entry, which is how a release that follows AniList
rather than TVDB numbers its seasons. `EpisodeGuide::get` tries, in order:

1. the season and number the file states;
2. for a file in no season or season one, the provider's own absolute list,
   then season one;
3. the absolute number — but only into the season the file states, so an
   absolutely-numbered season folder (`Attack on Titan/S2/… - 26`) finds
   S2E1, and `S03E13` can never be answered with season one's 13th;
4. the entry numbering, only for a season TVDB does not have at all — which is
   what makes Mushishi `S03E01` *Zoku Shou 2* episode 1 while Mushoku Tensei
   `S02E01`, a season TVDB does have, stays TVDB's.

`streamingEpisodes` is gone.

**Enrichment is recorded, empty or not.** A title's episode list and its
fanart are one *enrichment*, stored together by `Catalog::set_enrichment` in one
transaction that always replaces the old episode rows — so an empty answer
clears what a previous match left behind. `title_metadata.enriched_at` records
that it was asked, which is what keeps a film with no episodes from being
re-asked on every match run. A new match clears it; a failed enrichment leaves it
clear, so the title is asked again.

**No banner on a 16:9 tile.** AniList's `bannerImage` is no longer stored as a
backdrop. The backdrop is ani.zip's fanart where there is one; otherwise the tile
letterboxes the poster.

**Schema v9** adds the three numbering columns and `enriched_at`, deletes every
stored AniList episode row and every AniList banner-as-backdrop — all of it
`streamingEpisodes` data and wrong-shaped art — and leaves every title
un-enriched, so the next match run (which the app starts on its own) refills
them. Matches, hand-picked ones included, are kept.

## Phase 2 — art and order (done)

**More of the answer kept.** The AniList query now also asks for the full
start date, broadcast season, banner, popularity, status, the next airing
episode, spoiler-free tags ranked 60 or more, main studios, the `Director`
among the most relevant staff, and the relations that keep to one story
(`PREQUEL`, `SEQUEL`, `PARENT`, `SIDE_STORY`, `SUMMARY`, `ALTERNATIVE`,
`COMPILATION`, `CONTAINS` — not `SPIN_OFF`, `CHARACTER` or `OTHER`, which reach
into other franchises). All of it lives in `TitleDetails`, stored as one JSON
column (**schema v10**). A match stored before that is fetched again *by id*
(`Work::Refresh`) — never searched for, so hand-picked matches stay — and a
refresh of the same entry keeps its episodes, fanart and enrichment record.
Enrichment also stores the chain of entries it walked.

**Art.** Three textures per title where there was one: the tile's (`Art::of`),
the 2:3 cover (`Art::poster`), and wide art (`Art::wide` — fanart, else the
AniList banner) decoded at up to 1920 px for the library's hero and the title
page's band. The banner, wrong in a 16:9 tile, fits those strips as drawn.

**Poster grid.** Settings → Appearance → *Library tiles*: covers (the default)
or 16:9 stills. Covers fill their tile; a cover in a still-shaped tile is still
fitted whole.

**Names.** Settings → Appearance → *Title names*: the share's, English or
romaji. Search matches all of them whichever is shown.

**Sorts.** A–Z by the shown name, recently watched, recently added (from
`node_first_seen`, which survives recrawls; files present before v10 count as
added at 0), newest by air date, highest rated, most popular.

**Franchises.** `pstr_core::franchise::group` joins titles that share a
provider id — their own, a related one, or one their episode walk passed
through. The grid folds each franchise into its lead (the first series to air,
else the first film) with a `+N` badge; the stack button beside the sort turns
that off. The title page shows the whole franchise in release order. On the
library this was built against: Evangelion ×4, Made in Abyss ×4, Heaven's Feel
×3, Mushishi with its OVA, Jujutsu Kaisen with *0*, Chainsaw Man with *Reze*.

**Shelves.** `pstr_core::shelves::pick`: airing now, then directors, studios
and genres, each only when it spans three *franchises* — Made in Abyss's four
titles are one director's, and that is not a shelf — and a genre only under 40%
of the library. Four at most, over the unfiltered library. Here: *Directed by
Satoshi Kon*, *From MADHOUSE*, *From MAPPA*, *From Production I.G*.

**The title page** adds the season, studios, director and next airing episode
to its line of facts, and the tags beside the genres.

Not done: Android shows none of this yet beyond what it already drew — the
bridge carries `TitleDetails` through a hand-picked match, and the grouping,
shelves and sorts are in `pstr-core` ready for it, but its screens are
unchanged. TMDB fills no details.

## Phase 3 — reach

- **Android**: the sorts, grouping, shelves and names above.

- **Local matching** against the offline database, so only ids leave the
  machine.
- **Routing by title**: AniList first, then TMDB (with a key) or TVmaze for
  whatever is not anime — Game of Thrones in a mostly-anime share.
- **Folders of films** matched per file: four Rebuild films in one folder are
  four entries, not one.
- **Skip intro** from AniSkip for files without chapters, and filler flags from
  Jikan.
