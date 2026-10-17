# Account and sync

Signing in to Proton is optional. Without it, the app plays public share links and keeps
everything on one device. Signing in adds two things:

- **Folders of your own Drive** as shares, with no link to copy.
- **Sync** of your place in every episode, your shares and your settings, across every device
  you sign in on.

## Signing in

The **Proton account** section is at the top of the **Shares** page.

1. Enter your Proton address and password, then **Sign in**.
2. If your account has two-factor authentication, enter the code from your authenticator app.
3. If your account has a separate mailbox password, enter it and choose **Unlock**.

Proton sometimes asks to confirm you are human. The app then opens a small window with Proton's
own CAPTCHA page. Solve it, and sign-in continues.

The app keeps a session, not your password. The session goes into the system keyring. **Sign
out** ends it on Proton's side and deletes it here.

Not supported yet:

- **Security keys** (FIDO2). An account whose only second factor is a security key cannot sign
  in. Add an authenticator app as a second method in your Proton settings.
- **Email or SMS verification** instead of a CAPTCHA.

## Adding a folder of your Drive

1. Sign in.
2. Under **Add from your Drive**, choose **Browse your Drive**.
3. Open folders until you reach the one that holds your videos.
4. Choose **Add this folder**.

The folder becomes a share like any link: it is crawled, shown in the library and streamed. It is
read through your account, so there is no link or password to store. The same folder has the
same identity on every device, so watch history follows it.

## Sync

When you are signed in, each device keeps a small file in your Drive. You find them under
**Computers** on the Proton Drive website, in a device called `proton-stream`, in the folder
`watch-history`.

**What syncs:**

- Where you stopped in each episode, and which episodes you watched
- Your shares, including their links and passwords (end-to-end encrypted, like every file in
  your Drive)
- Settings: playback, appearance and posters and descriptions, including a TMDB key
- Matches you picked by hand, and your audio and subtitle choice for each show

**What stays on each device:** volume and mute, the streaming cache size, text size, Wi-Fi only,
background audio, hardware decoding, window size, and the library's filter and sort order.

**When it syncs:** when the app starts, every 5 minutes while it is open, when you stop playing,
and when it closes. **Sync now** runs it at once. Android also syncs in the background shortly
after you leave the app.

**How conflicts are settled:** each device writes only its own file, and reads everyone else's.
For each episode and each setting, the newest change wins. A device that signs in for the first
time takes the others' state and does not overwrite it with its own defaults.

A device that has not synced for 90 days has its file moved to the Drive trash.

The status line under your address says how the last sync went, for example *Synced 3 min ago ·
2 positions from other devices*. To stop syncing, sign out.
