# proton-stream

A Netflix-style desktop client for **Proton Drive public links**.

Paste a share URL and its password; get a browsable, streamable library. No
Proton account, no server to host, no download step — episodes start playing in
seconds and the seek bar works.

Built on [`proton-sdk-rs`](https://github.com/narrrl/proton-sdk-rs), a pure-Rust
Proton Drive SDK.

## Why it works

A Proton Drive file's content blocks are each a self-contained PGP packet
decrypted under one shared session key. There is no sequential streaming session
to resume, so seeking costs exactly the blocks the seek lands on — a 2 GB episode
is playable without downloading 2 GB of it.

## Status

Early, but it plays, and there is a window now. The SDK streaming surface, the
catalog, the streaming layer and the player are done and verified against a real
share; the app has a library wall, a page per title with seasons and episodes,
resume-where-you-left-off, a transport bar and share management. The picture
still lands in mpv's own window — embedding it in the app's surface is next. See
`docs/DEVELOPMENT.md`.

```bash
cargo run --release -p pstr-app          # the app
```

Working today, via the `pstr` CLI:

```bash
cargo run -p pstr-cli -- add --name anime 'https://drive.proton.me/urls/TOKEN#fragment'
cargo run -p pstr-cli -- crawl
cargo run -p pstr-cli -- list
cargo run --release -p pstr-cli -- play --file Frieren --disk-cache
cargo run --release -p pstr-cli -- bench --file Frieren --verify
```

On a 761 MiB, 26-minute 1080p HEVC episode over a real password-protected link:
**2.3 s to first frame, ~1.5 s to resume after dragging to an arbitrary point,
61 MiB fetched for the first 38 seconds of playback.** Watch it again and the
disk cache serves it with **no network at all** — 0.3 s to first frame.

`bench` measures the same file without a player, reporting cold seek latency,
sustained throughput and peak memory, and `--verify` cross-checks a mid-file read
against the SDK's own range download.

## Design

| | |
|---|---|
| Topology | Fat client. No server; each viewer's app talks to Proton directly. |
| Auth | Public links only — but several, merged into one library. |
| Player | Embedded libmpv: HEVC 10-bit, HDR and ASS subtitles with hardware decode. Volume, audio-track, subtitle and chapter pickers; skip-the-opening; next/previous episode and autoplay. The language picked is preferred for the next file too. |
| Transcoding | None. mpv demuxes and decodes natively. |
| UI | egui/eframe on glow; mpv renders into the same GL context. |
| Themes | The shipped near-black palette plus Catppuccin Latte, Frappé, Macchiato and Mocha, each with a choice of eight accents — pink and sky among them — drawn as a gradient unless you turn that off. Picked on the Shares page, applied without a restart. |
| Metadata | Filename parsing, with optional AniList/TMDB enrichment — **off by default**, because enabling it sends your library's titles to a third party. |
| Platforms | Linux and Windows. |

## Building

Rust 2024, MSRV 1.96.

```bash
cargo build --release
```

`pstr-player` needs libmpv development headers (`mpv` ≥ 2.0). On Windows,
`libmpv-2.dll` is bundled next to the executable.

## Privacy

- The share URL fragment **is** the decryption password. It and any custom
  password live in the OS credential store — Secret Service on Linux, Credential
  Manager on Windows — never in a config file.
- Nothing leaves your machine except requests to Proton, unless you turn metadata
  enrichment on. The player is fed through an in-process mpv stream callback, not
  a local HTTP server: there is no port for another process on the machine to ask
  for the plaintext, and mpv is handed an opaque token rather than a URL that
  would put a share secret in its log and window title.
- The block cache holds decrypted content. It lives in your cache directory and
  can be deleted at any time the app is not running.

## License

MIT.
