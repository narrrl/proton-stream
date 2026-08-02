# Testing

## Offline

Everything in `pstr-core` is offline and pure. That is deliberate: the filename
parser, the config writer and the catalog migrations are exactly the things whose
edge cases are painful to reproduce against a live share, so they are pinned by
table-driven tests instead.

```bash
cargo test --workspace --locked
cargo test -p pstr-core naming::            # just the parser
```

What is pinned today:

- **`naming`** — the fansub, scene and cross conventions; dashes inside titles;
  version suffixes; a bracketed resolution that must not read as a year; a word
  starting with `s` that must not start a season; dotted titles vs. extensions;
  and that an unrecognisable name still yields a usable title rather than being
  dropped.
- **`config`** — round trip, missing-is-`None`, **unparseable-is-an-error** (the
  one that protects the share list from a read-modify-write wiping it), and no
  temp file left behind.
- **`catalog`** — schema version, refuse-to-open on a newer schema, share replace
  semantics (drops removed rows, leaves other shares alone), and that **a recrawl
  preserves watch state**.

`pstr-stream` is offline too, and for the same reason: it runs against a
`BlockSource` fake, so the caching, deduplication and read-ahead behaviour that
is nearly impossible to provoke deliberately over a network is pinned exactly.

- **`block`** — byte range → block mapping, including non-uniform sizes, a range
  ending exactly on a boundary (which must not pull the next block), and splicing
  into the right part of the output buffer.
- **`ring`** — never exceeds its byte budget, evicts least-recently-*used*, a
  containment check records no traffic and no recency, a block bigger than the
  budget is not stored, reinserting does not double-count.
- **`disk`** — round trip, a block disagreeing with its sidecar is rejected *and
  removed*, a block with no sidecar is a miss, eviction deletes the files rather
  than merely forgetting them, a reopened cache adopts the previous run's bytes,
  and cache paths carry no names.
- **`single_flight`** — N callers run the work once, a waiter sees the leader's
  failure, a failed call leaves the key retryable, and a **cancelled** leader does
  not wedge the key.
- **`stream`** — reads across block boundaries, a seek fetches only the block it
  lands in, a demand read joins an in-flight prefetch instead of refetching, a
  failing prefetch never fails the read, **a seek cancels the read-ahead it
  invalidates** while sequential playback cancels none, and streaming a file far
  larger than the ring stays inside the budget.

`pstr-player` is offline too, everywhere it can be. mpv itself needs a window and
a codec, but nothing mpv's stream callbacks *decide* lives in the callbacks —
they hand straight to `StreamCursor`, a plain safe type over a `BlockSource`
fake, which is what the tests drive.

- **`cursor`** — sequential reads walk the file and stop at EOF, a seek moves
  where the next read starts, a read crossing a block boundary is allowed to come
  up short but returns the right bytes, seeking exactly to EOF is allowed while
  past it is an error, **mpv's post-open seekability probe to 0 succeeds** (fail
  it and every seek bar is gone), and a cancelled stream fails its reads and
  stays failed.
- **`registry`** — a published stream is reachable through its URL, dropping the
  handle revokes it, and **a token is never reused** after the stream behind it
  is gone, so a stale URL cannot start playing whatever took the number over.
  Only well-formed `pstr://` URLs parse.
- **`player`** — every mpv end-file reason maps to something the UI can act on
  (`Eof` means "next episode"; nothing else does), and an observed property
  arriving in an unexpected format is *not* read as a position of zero.

## Against a real share

Needs a Proton Drive public link. Use a share you can afford to have indexed;
nothing here writes, but the crawl reads every node's metadata.

```bash
cargo run -p pstr-cli -- add --name anime 'https://drive.proton.me/urls/TOKEN#fragment'
cargo run -p pstr-cli -- add --name anime 'https://…' --custom-password   # prompts
cargo run -p pstr-cli -- crawl
cargo run -p pstr-cli -- list
```

Check: node count matches the folder, titles and episode numbers look right, and
the crawl time is seconds rather than minutes (if it is minutes, the SDK's
node-key cache is not doing its job).

The reference run, against a real library: **354 nodes crawled, 310 playable, 22
titles, ~25s**. Eleven files end up with no episode number and that is correct —
they are films and OP/ED extras. Anything else unnumbered is a parser gap; add
the filename to `naming`'s tests before fixing it.

Useful for spotting gaps:

```bash
sqlite3 ~/.local/share/proton-stream/catalog.db \
  "SELECT title, name FROM nodes WHERE is_folder=0 AND episode IS NULL;"
```

## Benchmarking playback, without a player

`pstr bench` reads a catalogued file the way a demuxer would — 64 KiB at a time —
and reports the three numbers that decide whether the app is usable: cold seek
latency, sustained throughput, and peak resident memory.

```bash
cargo run -p pstr-cli -- bench --file Frieren --verify
cargo run -p pstr-cli -- bench --file Frieren --readahead 0      # isolate read-ahead
cargo run -p pstr-cli -- bench --file Frieren --disk-cache       # run twice
```

`--verify` cross-checks a mid-file read against the SDK's own `download_range`.
Run it whenever anything in the block-mapping or caching path changes; it is the
only check that catches bytes served from the wrong offset, which otherwise
surfaces as video that decodes into garbage rather than as an error.

Reference run, 761 MiB episode, 191 blocks, over a real public link:

| | cold seek (worst) | sustained | peak resident |
|---|---:|---:|---:|
| read-ahead off | 572 ms | 7.2 MiB/s | 60 MiB |
| read-ahead 6, no seek cancellation | **2895 ms** | 5.0 MiB/s | 128 MiB |
| read-ahead 12, seek cancellation | 715 ms | 8.7 MiB/s | 128 MiB |
| read-ahead 32 | 716 ms | 7.8 MiB/s | 128 MiB (evicting) |
| warm disk cache | **14 ms** | 1039 MiB/s | 76 MiB |

Two findings worth not rediscovering. Read-ahead left running from the position
the viewer just left holds exactly the bandwidth the seek needs — 5x on
worst-case latency. And a read-ahead window deeper than half the ring evicts its
own blocks before the player reaches them, so throughput *falls*. Both are fixed
in `pstr-stream::stream`; both are pinned by offline tests.

Expect run-to-run spread of 15–20% on the throughput number. Treat a change under
that as noise.

Note that `bench`'s tuned read-ahead default (12) is **not** the right one for
mpv, which brings its own — see `docs/BUGS.md` B8. A change that improves `bench`
has not thereby improved playback; measure both.

## Playing, with a player

```bash
cargo run --release -p pstr-cli -- play --file Frieren
cargo run --release -p pstr-cli -- play --file Frieren --start 900       # cold mid-file seek
cargo run --release -p pstr-cli -- play --file Frieren --disk-cache      # run twice
cargo run --release -p pstr-cli -- play --file Frieren --headless --for-seconds 60
```

Use `--release`. A debug build is watchable but the numbers below are not
reproducible from one.

`--headless` decodes with no window and no sound, so the whole chain still runs
somewhere there is no display; `--for-seconds` ends an unattended run without a
window to close. Together they are the smoke test. Every run prints time to first
frame, seek-resume latency, and what the block layer actually fetched.

Reference run, same 761 MiB / 1560 s episode over a real public link, defaults:

| | cold | warm disk cache |
|---|---:|---:|
| time to first frame | 2.3 s | **0.3 s** |
| mid-file seek resumed in | 1.0–1.6 s | — |
| blocks fetched for 38 s of playback | 16 (61 MiB) | **0** |
| ring evictions | 0 | 0 |

Sustained playback holds exactly realtime: 178 s of media in 180 s of wall clock,
the 2 s deficit being the time to first frame, with no drift over the run. Check
that: a deficit that *grows* is the block layer failing to keep up, and it is the
one failure the smoke test can see but a short run cannot.

`Cannot load libcuda.so.1` on stderr is mpv's `hwdec=auto-safe` probe declining a
backend. Not an error.

## SDK-side

The streaming surface this app depends on is tested in `../proton-sdk-rs`:

```bash
cd ../proton-sdk-rs
PROTON_TOTP_SECRET=... cargo test -p proton-drive-rs --test live_sharing \
  -- --ignored --nocapture --test-threads=1
```

`a_visitor_can_seek_within_a_shared_multi_block_file` is the load-bearing one —
it verifies that the `drive/unauth/` revision endpoint returns a usable `XAttr`,
without which block boundaries cannot be placed and seeking is impossible.

Live runs there must stay `--test-threads=1`: Proton throttles repeated logins
and each fresh one burns a TOTP window.

## Acceptance, once playback lands

1. Add the share; the catalog populates with correct titles and seasons.
2. Play a 1080p HEVC episode — first frame in a couple of seconds, not a full
   download.
3. Drag the seek bar to ~75%; playback resumes there within a block fetch.
4. Close and reopen; the resume position is honoured.
5. Leave it paused past the anonymous session's lifetime, then seek — the
   session renews rather than erroring.
6. Watch the same episode again; it comes from the disk block cache.
7. Repeat on Windows with the bundled `libmpv-2.dll`.
