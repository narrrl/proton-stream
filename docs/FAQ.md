# FAQ

## Is this an official Proton app?

No. It is an unofficial, open-source client written by one developer. Proton AG has not reviewed,
endorsed or supported it. "Proton" and "Proton Drive" are Proton AG's trademarks.

## Do I need a Proton account?

No, not to watch a public share link. You need one to add folders of your own Drive and to sync
your place across devices. See [Account and sync](ACCOUNT_AND_SYNC.md).

## Does it download the whole file before playing?

No. Proton Drive stores a file as blocks that each decrypt on their own, so the app fetches only
the blocks around the point you are watching. A seek fetches the block it lands on. On a
26-minute 1080p episode the first frame takes about 2.3 seconds.

## Does it transcode?

No. The video plays as it is stored, through libmpv. HEVC 10-bit, HDR and ASS subtitles all work,
with hardware decoding where the device has it.

## Does it provide any videos?

No. It plays what you keep in Proton Drive or what someone shares with you. It has no catalogue,
no search of other people's shares and no content of its own.

## Why are there no posters?

Posters and descriptions are off by default, because looking them up sends your title names to
AniList or TMDB. Turn them on in **Settings → Posters and descriptions**. See
[Privacy](PRIVACY.md).

## A title is matched to the wrong show. How do I fix it?

Open the title and choose **Match…** (or **Change match**). Search for the right entry and pick
it. A match you pick is never replaced, and it syncs to your other devices.

## The episodes of one show appear as separate titles.

The app takes the show's name from its folder. Put every episode of a show in one folder, or in
season folders below it. [How the library reads your
folders](USER_GUIDE.md#how-the-library-reads-your-folders) has examples.

## Can several people share one library?

Yes. Send them the same share link. Each person's app keeps its own watch history; sync only
connects devices that sign in to the same Proton account.

## Is there a macOS version?

Not as a release. The app builds and runs on macOS from source, and `scripts/build.sh` can make a
`.app` and a `.dmg` on a Mac, but no release ships one yet.

## Can I cast to a TV?

Not yet. On an Android TV, install the APK and run the app on the TV itself.

## Where do I report a bug?

Open an issue on [GitHub](https://github.com/narrrl/proton-stream/issues). Say which platform
and version you use, and what you did.
