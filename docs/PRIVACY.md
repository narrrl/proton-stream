# Privacy

proton-stream has no server and no account with its developer. Each app talks to Proton directly,
and to nothing else unless you turn on posters and descriptions. This page lists what goes where.

## What the app sends, and to whom

| To | What | When |
|---|---|---|
| Proton | Requests for the shares you added and, if you signed in, your account's Drive | Always |
| AniList and ani.zip, or TMDB | The names of the titles in your library, and the ids of matched titles | Only with **Posters and descriptions** on |
| TVmaze | With AniList chosen: the names of the titles AniList has no match for, and the ids of those TVmaze matched | Only with **Posters and descriptions** on |

Nothing else leaves your device. There is no telemetry, no crash reporting and no update check.

**Posters and descriptions** are off by default. When they are on, the provider learns the names
of the titles in your library. It does not learn your files, your watch history or your shares.
Turning them off deletes every stored answer.

## Secrets

- **The part of a share link after `#`** is the key that decrypts the share. The app treats it as
  a password.
- **A link's own password**, if it has one.
- **Your Proton session**, if you signed in. The app keeps the session, not your password.

All of these go into the system's credential store, never into a config file:

| Platform | Store |
|---|---|
| Linux | Secret Service (GNOME Keyring, KWallet, KeePassXC) |
| Windows | Credential Manager |
| Android | App storage, encrypted with a key in the Android Keystore |
| iOS | Keychain, this device only |

If you sign in, your shares, with their links and passwords, also sync through your own Drive.
Like every file there, they are end-to-end encrypted. See [Account and sync](ACCOUNT_AND_SYNC.md).

## How the player gets the video

The player is fed through a callback inside the app's own process. There is no local web server,
so there is no port another program on the computer could ask for the decrypted video. The
player is handed an opaque token, not a link, so no secret ends up in its log or window title.

## What stays on your device

| What | Where on Linux |
|---|---|
| Settings and the share list (without secrets) | `~/.config/proton-stream/` |
| The library and watch history (`catalog.db`) and downloads (`offline/`) | `~/.local/share/proton-stream/` |
| Streamed video (`blocks/`), thumbnails and posters | `~/.cache/proton-stream/` |

On Windows the same folders are under `%APPDATA%\narl\proton-stream\` and
`%LOCALAPPDATA%\narl\proton-stream\`. On macOS they are under `~/Library/Application Support/`
and `~/Library/Caches/`, in `io.narl.proton-stream`.

**The streaming cache and downloads hold decrypted video.** Anyone who can read your user's files
can play them. Empty the cache in **Settings → Storage**, and delete downloads on the
**Downloads** page.

The library database holds file names and your watch history, in plain text. It holds no
secrets.

## Other people on the same computer

- A share link given on the command line, as in `proton-stream 'https://…#key'`, is visible in
  the process list while the command runs, and may land in your shell history. Paste links into
  the window instead.
- Your desktop's media controls are given the title, the episode and a picture already on disk.
  They are never given a provider link, so the desktop does not contact a provider by itself.
