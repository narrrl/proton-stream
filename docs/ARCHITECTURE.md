# Architecture

## The problem

A Proton Drive public link is normally a download link. This app turns one into a
streamable library.

Nothing about the crypto prevents that: a file's content blocks are each a
self-contained PGP SEIPD packet decrypted under one shared session key, so there
is no sequential streaming session to resume and seeking costs exactly the blocks
the seek lands on. What was missing was plumbing — `RevisionReader`, the SDK's
random-access reader, was welded to the *authenticated* client. Undoing that weld
(`proton-sdk-rs` 0.3.3, `transport.rs`) is what made this app possible.

## Crates

```
pstr-app (bin: proton-stream)     pstr-cli (bin: pstr)
     │                                  │
     ├── pstr-player ── libmpv          │
     │       │  stream_cb + render API  │
     │       ▼                          │
     └── pstr-stream ───────────────────┤
             │                          │
             └── pstr-core ─────────────┘
                     │
              proton-drive-rs ──▶ proton-sdk
```

- **`pstr-core`** — the share list and its secrets, the catalog database, the
  filename parser, and the crawl. Knows nothing about playback or pixels.
- **`pstr-stream`** — `RevisionReader` to a seekable byte stream: reader LRU,
  in-memory block ring, forward read-ahead, optional on-disk block cache. Only
  its `reader` module touches the SDK; everything else runs against a
  `BlockSource` trait, which is what makes the caching and read-ahead testable
  with no account and no network.
- **`pstr-player`** — libmpv. Fed through a custom `pstr://` protocol registered
  with `mpv_stream_cb_add_ro`; rendered through `mpv_render_context_create` with
  `MPV_RENDER_API_TYPE_OPENGL` into eframe's glow context. The URL is an opaque
  token into a registry, never an identity: the caller opens the stream in async
  code where a failure can be reported, then publishes it. mpv's open callback is
  a hash lookup that cannot block and cannot leak a share secret into a log line.
  Everything the callbacks decide lives in a safe, tested `StreamCursor`; the
  `unsafe` is confined to one module of pointer plumbing. What is inside the file
  — audio and subtitle tracks — is read from `track-list` one sub-property at a
  time (`tracks.rs`) and selected through `aid`/`sid`; the *language* of a choice
  is carried to the next file with `alang`/`slang`, which is what makes "Japanese
  audio" hold for the rest of a season.
- **`pstr-app`** — the egui UI. Three threads' worth of work kept apart: the UI
  thread only draws, an `Engine` runs everything with a network or a disk in it
  on the tokio runtime and answers with events, and a player thread owns mpv for
  the length of a file. Pages are pure functions of state that push `Action`s
  rather than mutating, which is what allows a click to change the page it was
  drawn on.
- **`pstr-cli`** — the headless front end, and the thing that tells you which
  layer is broken.

## Data flow, playing an episode

```
click                      egui
  │
  ▼
pstr://<share>/<link_id>   mpv opens the URL, calls back on its demuxer thread
  │
  ▼
VideoStream::read_at       pstr-stream: ring? disk cache? else fetch
  │
  ▼
RevisionReader::read_at    SDK: the blocks overlapping the range, 10 at a time
  │
  ▼
get_storage_blob           block storage, pm-storage-token, no session credential
  │
  ▼
ContentKey::decrypt_block  one PGP packet per block, on the blocking pool
```

The demuxer thread is allowed to block, so the `stream_cb` callbacks bridge into
a dedicated tokio runtime with `Handle::block_on`.

Two properties of that middle layer are load-bearing enough to state here, both
established by measurement rather than reasoning (`docs/TESTING.md`):

- **A seek cancels outstanding read-ahead before it fetches anything.** Otherwise
  the prefetches from the abandoned position hold the bandwidth the seek needs —
  worth 5x on worst-case seek latency.
- **Read-ahead never runs deeper than half the ring.** A window larger than the
  cache it lands in evicts its own blocks before the player reaches them.
- **How far to read ahead belongs to the consumer, not to the block layer.** mpv
  buffers ahead of the picture itself, so under mpv the block layer reads ahead
  *not at all*; a second speculative layer only takes bandwidth from the reads
  mpv is blocked on. A reader with no buffer of its own still wants twelve.

Every block is fetched exactly once no matter how many readers want it: the
open, and each block fetch, go through a single-flight keyed on the block. The
demand read and the prefetch racing for the same block is the normal case, not
an edge case.

## Trust and secrets

- The share URL fragment **is** the decryption password, and a custom-password
  link has a second secret. Both live in the OS credential store; `shares.json`
  holds only the id, display name and token.
- Nothing this app holds can write to the share: a viewer link grants
  `MemberRole::Viewer`, and the SDK's public-link write side is unported anyway.
- Signature verification is absent on the visitor path by design — an anonymous
  visitor cannot read `core/v4/keys/all` to resolve a signer. Decryption still
  proves the bytes were encrypted to the node key. Nodes come back with nothing
  *claimed* rather than a verification that silently passed.
- Metadata enrichment is off by default because enabling it sends the user's
  library titles to a third party.

## Persistence

| Store | Path | Rebuildable? |
|---|---|---|
| Share list | config dir, `shares.json` | **No** — atomic writes, never overwritten when unparseable |
| Share secrets | OS credential store | **No** |
| Catalog | data dir, `catalog.db` | Yes — recrawl |
| Watch state | same DB, own table | **No** — deliberately survives a recrawl |
| Block cache | cache dir, `blocks/` | Yes — must work correctly after deletion |
| Poster thumbnails | cache dir, `thumbs/` | Yes — refetched per file, one small decrypt each |
