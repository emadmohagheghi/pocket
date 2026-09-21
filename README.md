# Pocket

Pocket is a lightweight, privacy-first companion app for Windows and Linux for
capturing selected text and voice notes without leaving your current workflow.

It is built with Rust, Tauri v2, React, TypeScript, shadcn/ui, and Tailwind CSS.
Pocket is currently in early development. Release builds target Windows x64
(NSIS installer) and Linux x64 (`.deb` and `.AppImage`).

## Features

- Capture selected text from any application with a double press of Left Shift
  (Windows). The text is saved straight into the active workspace — no window
  opens; a brief on-screen confirmation is shown instead.
- Record voice notes from the main window or the system tray, with a waveform
  scrubber for precise seeking.
- Write notes in markdown, including rich paste from HTML (from browsers,
  chat apps, and so on), with inline links that open on hold-Ctrl-and-click.
- Organize text and recordings into independent workspaces.
- Search the active workspace as you type.
- Mark text and voice notes as done, todo-style.
- Select multiple entries to copy them, copy them as a numbered list, or
  delete them — with single-step undo. Merging notes folds their text into
  the oldest note, moves its images along, and releases embedded voice notes
  back to the feed; media files are never lost and undo reverts the whole
  merge at once.
- Export and import complete backup archives, including audio files.
- Optionally start Pocket automatically at login.
- Keep all application data local, with no account, cloud service, telemetry,
  or advertising.

## Privacy-sensitive behavior

Pocket uses a Windows low-level keyboard hook to recognize the double-Left-Shift
capture gesture. Key contents are not logged, stored, or sent over the network.
The hook only keeps the timing and side of Shift presses and whether another
key interrupted the gesture.

When the text-capture gesture is used, Pocket temporarily uses the Windows
clipboard to copy the text currently selected in the foreground application.
This action replaces the clipboard's previous contents. The captured text is
saved locally only when the selection is non-empty.

Microphone access is used only for a recording explicitly started by the user.
Voice recordings remain on the local machine unless the user chooses to export
a backup.

See the full [Privacy Policy](PRIVACY.md) and [Security Policy](SECURITY.md).

## Capture gesture

Press Left Shift twice, in any application, to save the selected text straight
into the active workspace (Windows only). In-app shortcuts are shown next to
their actions in the app's menu. All shortcuts match physical keys, so they
work on any keyboard layout.

## Installation

Official installers are published on the GitHub Releases page:

- Windows: an NSIS `.exe` installer for the current user.
- Linux: a `.deb` package (Debian/Ubuntu) and a portable `.AppImage`.

Release artifacts are currently unsigned while the project prepares its
application to SignPath Foundation. Windows SmartScreen may therefore display
an unknown-publisher warning. Never download Pocket installers from an
unofficial source.

## Local data

Pocket first attempts to create a `PocketData` directory next to the executable.
If that location is not writable, it uses the per-user application configuration
directory instead. The exact active location is shown in Settings.

```text
PocketData/
  settings.json
  workspaces.json
  workspaces/<id>/workspace.json
  voices/<id>/<recording>.webm
  images/<id>/<image>.<ext>
```

Writes to Pocket's JSON data files are atomic. Media playback (voice and
images) uses restricted local protocols that only expose files registered in
workspace metadata, with HTTP range support so voice seeking never re-downloads
the file.

## Development

Requirements:

- Windows 10/11 x64 or Linux x64
- Node.js and pnpm
- Rust stable (MSVC toolchain on Windows)
- Microsoft Edge WebView2 Runtime (Windows) or `libwebkit2gtk-4.1` (Linux)

```powershell
pnpm install
pnpm tauri dev
pnpm test
pnpm build
cargo test --manifest-path src-tauri/Cargo.toml
pnpm tauri build
```

See [CONTRIBUTING.md](CONTRIBUTING.md) before submitting a change.

## Code signing policy

Pocket is preparing to use SignPath Foundation for verifiable Windows release
signing. The complete policy, maintainer roles, and release guarantees are in
[CODE_SIGNING_POLICY.md](CODE_SIGNING_POLICY.md).

Free code signing provided by SignPath.io, certificate by SignPath Foundation.

Until the SignPath application is approved and the release workflow is updated,
official artifacts must be treated as unsigned.

## Credits

Pocket's capture concept and visual design are inspired by
[Copper](https://shadcn.com/copper), shadcn's capture app for macOS. Pocket is
an independent implementation focused on Windows and Linux and is not
affiliated with shadcn.

## License

Pocket is licensed under the [MIT License](LICENSE).
