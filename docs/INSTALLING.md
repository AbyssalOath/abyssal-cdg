# Installing

Every [GitHub release](https://github.com/AbyssalOath/abyssal-cdg/releases/latest)
includes installers for each platform - no Rust toolchain or build step, and
nothing else to install separately. Vocal removal, video export, and the
ONNX Runtime/ffmpeg they depend on are all bundled in (auto-align's
per-language speech models download automatically the first time you use
one).

| Platform | Download | First launch |
|---|---|---|
| **Windows** | `.msi` or `.exe` installer | SmartScreen may warn - see below |
| **macOS** | `.dmg` | Gatekeeper will warn - see below |
| **Linux** | `.AppImage` or `.deb` | AppImage: mark it executable first |

## Linux: make the AppImage executable

Plain downloads (browser, `curl`, `wget`, ...) can't carry a file's Unix
permission bits, so *every* AppImage from *every* project needs this once:

```bash
chmod +x Abyssal-CDG-Creator_*.AppImage
```

The `.deb` doesn't need this - your package manager sets permissions on
install.

## macOS: "Apple could not verify... is free of malware"

The app isn't signed with a paid Apple Developer ID or notarized by Apple
(that program costs $99/year), so Gatekeeper warns on first launch - as it
does for any app downloaded outside the App Store that isn't notarized.
It's not a sign of anything wrong with the app. To open it anyway, either:

- right-click (or Control-click) the app in Finder, choose **Open**, then
  confirm **Open** (only needed once); or
- go to **System Settings → Privacy & Security**, scroll to the blocked-app
  notice, and click **Open Anyway**; or
- remove the quarantine flag in Terminal:

  ```bash
  xattr -d com.apple.quarantine "/Applications/Abyssal CDG Creator.app"
  ```

## Windows: "Windows protected your PC"

SmartScreen shows this for the same reason (no paid code-signing
certificate). Click **More info**, then **Run anyway**.

## Checking your download

Neither warning means the download was tampered with - it's the standard
message every unsigned indie app gets. Every release also publishes a
`SHA256SUMS.txt` next to the installers, so you can confirm your file
matches what CI actually built:

```bash
# Linux / macOS, in the folder with the download and SHA256SUMS.txt
sha256sum -c SHA256SUMS.txt --ignore-missing
```

```powershell
# Windows
Get-FileHash .\Abyssal-CDG-Creator_*.msi
```

This proves the file wasn't corrupted or swapped in transit, though it's
not a substitute for code signing. Everything is open source and built by
the same public CI workflow that produced the release - see
[SECURITY.md](../SECURITY.md) if you want to check what a release does
before running it.

## Building it yourself

See [BUILDING.md](BUILDING.md).
