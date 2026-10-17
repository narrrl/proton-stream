# Troubleshooting

## Adding a share

### "Proton share URL has no #password fragment; copy the full share link"

The link is missing the part after `#`, which is its decryption key. Copy the link again from
where you got it, all of it.

### "This link has a password of its own. Enter it to add the link."

The person who shared it also set a password. Turn on **The link asks for a password** (on a
phone, fill in **Custom password**) and type it.

### "That share is already added."

The link, or the same folder of your Drive, is already on the **Shares** page.

### "Last crawl failed: …"

The share could not be read. The link may have expired or been revoked, or its password
changed. If the password changed, choose **Re-enter link**.

## The keyring

### "share … has no stored credentials; remove and re-add it"

The link and password are gone from the keyring. This happens when a keyring is reset, or on
Android when the screen lock changes. Choose **Re-enter link** on the **Shares** page. Your
library, history and downloads stay.

### "credential store: …" on Linux

The app found no Secret Service. Install and start one, such as GNOME Keyring, KWallet or
KeePassXC with its Secret Service integration, and unlock it.

## Signing in

### "Proton did not accept that address and password"

Check both. Sign in on proton.me to make sure the account works.

### "Proton has blocked sign-ins from this app for now"

Proton's abuse protection stopped the sign-in. Wait a while and try again, or sign in once on
proton.me first.

### "This account's only second factor is a security key"

The app cannot use security keys yet. Add an authenticator app as a second method in your
Proton account settings, then sign in with its code.

### The CAPTCHA window does not open on Linux

The window needs WebKitGTK 4.1. The `.deb`, `.rpm` and Arch packages pull it in; with the
tarball, install it yourself.

### `pstr login` asks for a CAPTCHA

The command line cannot show one. Sign in once in the app.

## Playback

### Green blocks, a black picture or torn frames

Turn off **Hardware decoding** in Settings. It applies from the next file.

### An episode takes long to start

The first seconds of a file are fetched over the network. On a slow connection, start the
episode and give it a moment, or download it first. A second viewing comes from the disk cache.

### No skip button for the opening

The skip button comes from the file's chapter names. A file without named chapters gets none.

### The wrong subtitle track plays

Choose the track in the player. The app remembers it for the rest of the show. Set the
language to start in under **Settings → Playback**.

## Windows

### The app does not start from the portable `.zip`

Keep every file from the archive in one folder. The app needs `libmpv-2.dll` and
`WebView2Loader.dll` next to `proton-stream.exe`.

### SmartScreen warns about the installer

The binaries are not code-signed yet. Choose **More info**, then **Run anyway**.

### A pinned taskbar icon shows a blank picture after an update

Unpin it and pin it again once.

## Linux

### "Not responding" when the window is on another workspace

Fixed. On Wayland the app limits its own frame rate instead of waiting for vsync, which a
compositor withholds from a window it is not showing. Update to the latest release.

## Logs

Start the app from a terminal to see its log:

```bash
RUST_LOG=debug proton-stream
```

The same works for `pstr`. When you
[open an issue](https://github.com/narrrl/proton-stream/issues), include the lines around the
error, but **remove any share link first**: the part after `#` is a key.
