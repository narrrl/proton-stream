# Command-line reference

The desktop packages install two programs:

- `proton-stream`, the app
- `pstr`, a command-line tool that does the app's work without a window

Both use the same shares, library, settings and keyring, so a share added with `pstr` shows up in
the app.

## proton-stream

```bash
proton-stream                                              # open the app
proton-stream 'https://drive.proton.me/urls/TOKEN#KEY'     # open it with the Add form filled in
```

Only one window runs at a time. A second `proton-stream` brings the open window forward, hands it
any share link it was given, and exits.

A link given as an argument is visible to other users of the computer in the process list while
the command runs, and it may land in your shell history. Pasting it into the window with
**Ctrl+V** does neither.

## pstr

`pstr --help` lists every command, and `pstr <command> --help` explains one. `pstr` never takes a
password or key as an argument; it asks for them.

### Shares

| Command | Does |
|---|---|
| `pstr add --name NAME URL` | Add a public share link. Quote the URL, because of the `#`. |
| `pstr add --name NAME URL --custom-password` | The same, and ask for the link's own password |
| `pstr add-folder --name NAME VOLUME:LINK` | Add a folder of your Drive, by the id `pstr browse` prints |
| `pstr shares` | List the shares and their ids |
| `pstr remove ID` | Remove a share and delete its link and password from the keyring |

### Account

| Command | Does |
|---|---|
| `pstr login ADDRESS` | Sign in. Asks for the password, a two-factor code and a mailbox password as the account needs them. |
| `pstr logout` | Sign out and delete the stored session |
| `pstr account` | Show who is signed in |
| `pstr browse [VOLUME:LINK]` | List your Drive's top-level places, or one folder |
| `pstr sync` | Sync watch history now |

`pstr login` cannot show a CAPTCHA. If Proton asks for one, sign in once in the app instead.

### Library

| Command | Does |
|---|---|
| `pstr crawl [--share ID]` | Read every share, or one, into the library |
| `pstr list [--share ID]` | Print what the library holds |

### Posters and descriptions

| Command | Does |
|---|---|
| `pstr metadata status` | Show whether lookups are on, and which provider |
| `pstr metadata set --on --provider anilist` | Turn lookups on with AniList |
| `pstr metadata set --on --provider tmdb --api-key` | Turn lookups on with TMDB, and ask for the key |
| `pstr metadata set --off` | Turn lookups off, and delete every stored answer |
| `pstr metadata match [--force] [--limit N]` | Look up titles now. `--force` asks again about titles that already have an answer. |

### Testing playback

These two are for finding out which layer is slow or broken.

```bash
pstr bench --file Frieren --verify      # measure seek time, throughput and memory, no player
pstr play --file Frieren --disk-cache   # play in a plain mpv window
```

| Option | Does |
|---|---|
| `--file TEXT` | The first file whose name or title contains `TEXT` |
| `--share ID` | Look in one share only |
| `--disk-cache` | Keep decrypted blocks on disk, as the app does |
| `--readahead N` | Blocks to fetch ahead of the reader |
| `--ring-mib N` | Memory for blocks, in MiB (128 by default) |
| `--verify` | `bench` only: compare a read against the SDK's own download |
| `--throughput-mib N` | `bench` only: how much to read for the speed test (32 by default) |
| `--start SECONDS` | `play` only: start at this position |
| `--headless` | `play` only: decode with no window and no sound |
| `--for-seconds S` | `play` only: stop after this many seconds |

## Environment variables

| Variable | Does |
|---|---|
| `RUST_LOG` | How much both programs log to the terminal, for example `RUST_LOG=debug` |
| `PSTR_FRAME_CAP` | The app's frame rate limit on Wayland. 60 by default; `0` turns it off. |
