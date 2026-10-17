# Getting started

This page takes you from nothing to a playing episode. It takes a few minutes.

## 1. Install the app

Download the package for your system from the
[latest release](https://github.com/narrrl/proton-stream/releases/latest) and install it.
[Installation](INSTALL.md) has the commands for each platform.

## 2. Add a share

A *share* is a folder the app reads videos from. There are two kinds:

- **A public share link.** Someone sends you a Proton Drive link such as
  `https://drive.proton.me/urls/ABC123#xyz`. You need no Proton account for this.
- **A folder of your own Drive.** Sign in to your Proton account, then pick a folder. See
  [Account and sync](ACCOUNT_AND_SYNC.md).

To add a public link on the desktop:

1. Copy the whole link, including the part after `#`. That part is the key that decrypts the
   share, so the link does not work without it.
2. Open the **Shares** page, or press **Ctrl+V** anywhere outside a text field.
3. Paste the link into **Link**.
4. If the person who shared it set a password of their own, turn on **The link asks for a
   password** and type it.
5. Click **Add share**.

The app opens the link to check it before it saves anything. If the link is wrong, it tells you
why and stores nothing.

On Android and iOS, tap **Add share** on the **Shares** tab. On Android you can also share the
link to the app from your browser. [Android and iOS](MOBILE.md) has the details.

## 3. Wait for the library to fill

The app reads the share's folders. This is called a *crawl*. Titles appear in the **Library** as
they are found. A large share takes a minute or two.

The folder layout decides how titles are grouped. Each show should have its own folder:

```text
Sousou no Frieren/
  Season 01/
    S01E01.mkv
    S01E02.mkv
Paprika (2006).mkv
```

A file without episode numbers is shown as a film. [How the library reads your
folders](USER_GUIDE.md#how-the-library-reads-your-folders) has the full rules.

## 4. Press play

Open a title and click **Play**. The episode starts in a few seconds, and you can drag the seek
bar to any point. The app remembers where you stopped, so **Resume** takes you back.

## Next steps

- **Posters and descriptions.** Turn them on in **Settings → Posters and descriptions**. They are
  off by default, because the app then sends your title names to AniList or TMDB. See
  [Privacy](PRIVACY.md).
- **Your place on every device.** Sign in to Proton on each device, and your position in every
  episode follows you. See [Account and sync](ACCOUNT_AND_SYNC.md).
- **Keyboard.** Press **?** on the desktop for the list of shortcuts.
