# Desktop signing

The public GitHub release builds Linux, Windows and macOS clients from its exact
tag. Windows installers have no publisher certificate. macOS bundles use a local
ad-hoc signature (`-`), not an Apple Developer ID, and are not notarized. The
ad-hoc signature lets Apple Silicon execute the bundle; it does not establish a
trusted publisher or remove Gatekeeper warnings.

Download only from `https://github.com/mlimarenko/piping/releases`. Each desktop
asset has a `.sha256` checksum and `.source.json` provenance record.

## macOS 14 or newer

1. Download the DMG for Apple Silicon or Intel to match your Mac.
2. Open it and drag Piping into Applications.
3. Try opening Piping. If macOS blocks the unrecognized developer, open System
   Settings > Privacy & Security, find the Piping warning and choose Open Anyway.
4. Confirm the per-application prompt, then enter your Piping instance address.

Do not disable Gatekeeper globally. A checksum mismatch, malware alert or
damaged bundle is not the normal unrecognized-developer prompt: stop and
download a fresh copy from the release before proceeding. Organization policies
can forbid unnotarized applications; ask the administrator rather than bypassing
those policies.

## Windows

Windows 10/11 and Windows Server 2016 or newer are supported. SmartScreen may
show an unknown-publisher warning. Check the download source and checksum before
choosing More info > Run anyway, if organization policy permits it. The installer
includes WebView2 and installs it when needed.

## Developer certificates

A future Developer ID or Windows publisher certificate must be provisioned
through protected release credentials, never committed to source. Developer ID
distribution also needs notarization and a clean-machine installation check;
possessing a certificate alone is not proof that Gatekeeper accepts a download.
