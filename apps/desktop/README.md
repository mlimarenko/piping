# piping-desktop - Tauri 2 shell

Desktop client for Piping on Windows, macOS, and Linux.

The app is intentionally a thin native shell around a deployed Piping web
origin. Every ordinary launch opens a local instance chooser. The last used
service address is preselected, the saved list is shown, and the user confirms
that instance or picks another one. The chosen address is stored in the OS app
config directory and opened in a WebView. A managed launcher that passes
`--service-url` (or `PIPING_DESKTOP_SERVICE_URL`) still opens that service
directly; the installer seed only preselects the address in the chooser.
Saved instances are available from the address button in the title bar or the
Piping menu > Instances. Opening another instance leaves existing connection
windows running. Each service origin has its own persistent WebView data store;
detached connection windows share their originating instance's store.

This keeps the browser and desktop products on one source of truth: auth, REST,
WebSocket, SSH, RDP, VNC, file browser, and clipboard behavior all stay in the
deployed Piping web app and backend.

## Runtime model

- `bootstrap` window: local static UI from `apps/desktop/ui`.
- `main` and `detached-*` windows: undecorated single-WebView windows loading
  the configured Piping service directly.
- The deployed web app renders the 30px desktop title bar only when the Tauri
  initialization script marks the page as `window.__PIPING_DESKTOP__`.
- The bootstrap and configured service windows have only minimal shell-chrome
  Tauri command permissions; there are no filesystem or secret permissions.
- Hostnames without a scheme are normalized to HTTPS.
- The custom top bar exposes update check, about, instance selection, and
  window controls.
- `window.open` requests from remote content are allowed only for the configured
  originating window's service origin and are opened as additional native desktop
  windows, independent of the last selected instance.
- Downloads go to the operating system's Downloads folder. Existing files are
  not replaced: a numbered suffix is added when a name is already taken. The
  shell reports the saved location or a download error and does not open files
  automatically.

The stored config file is `app.json` under the per-platform configuration
directory described below. It holds `serviceUrl`, the last opened address, and
`services`, the saved instance list; it contains no credentials.

## Client state and configuration

The shell stores only its own preferences. Accounts, sessions and application
data belong to the deployed service and live in that instance's WebView store.
Every platform uses the same layout: `<platform config root>/piping/app.json`.

| What | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Shell config | `$XDG_CONFIG_HOME/piping/app.json`, or `~/.config/piping/app.json` when `XDG_CONFIG_HOME` is unset or relative | `~/Library/Application Support/piping/app.json` | `%APPDATA%\piping\app.json` |
| Per-instance WebView data | `<config dir>/webviews/<sha256 of the origin>/` | Same directory, plus a WKWebView data-store identifier derived from the origin (macOS 14 or newer) | Same directory; WebView2 keeps its user data there |

`PIPING_DESKTOP_CONFIG_DIR` replaces `<platform config root>/piping` with an
absolute directory for managed installations; the `webviews/` directory moves
with it. The bundle identifier still names the application package and its
updater, not the user's configuration path.

`app.json` is written by the instance chooser and by a successful address check.
`services` is the saved list, `serviceUrl` is the address the chooser preselects.
Removing a saved instance only edits that list: its WebView profile stays on
disk. To sign an instance out completely, remove its directory under
`webviews/`. The profile holds cookies, local storage and cache for that origin,
so treat a copy of it as secret. Detached connection windows reuse the profile of
the instance they came from; instances never share one.

A client upgraded from an earlier release imports the previous
`space.piping.desktop` directory into `piping/` at startup and keeps every saved
instance and WebView profile. `desktop-config.json` is merged into `app.json`
before the old file is removed. The import also runs when `app.json` already
exists (for example after a partial run or an installer pre-seed): missing
instances are appended and missing profiles are moved, while an existing
`serviceUrl` wins. A profile directory that exists on both sides is never mixed:
the old copy is preserved next to it as `space.piping.desktop.legacy-<timestamp>`
for manual recovery. After the migration only `app.json` is read.

Sign-in lifetime is decided by the server. The login form offers an
off-by-default "Remember me" ("Запомнить") option that keeps the session for the
instance's configured remember-device lifetime, 30 days by default; it is not
meant for shared devices. The shell does not create, store or refresh tokens of
its own.

Startup address precedence:

1. `--service-url <url>` or `--piping-service-url=<url>` opens that service
   directly.
2. `PIPING_DESKTOP_SERVICE_URL` does the same when no argument is given.
3. The installer seed `desktop-service-url.txt` next to the executable fills the
   config when no address is stored yet; it preselects the address and never
   bypasses the chooser. Windows NSIS writes it from `/SERVICE_URL=`. A Linux
   AppImage mounts read-only, so it cannot use this seed; write `app.json`
   instead.
4. Otherwise the chooser opens with the saved address preselected.

On Linux the process sets `WEBKIT_DISABLE_DMABUF_RENDERER=1` before WebKit starts
unless the variable is already set; export it explicitly before launch to
override that default. The shell writes no application log file: it reports
failures in the window or a dialog, and diagnostics go to stderr when started
from a terminal. The update check is manual (Piping menu) and reads the latest
public GitHub release; it never installs updates silently.

## Unattended installation

Every platform can be provisioned without interaction once the installation
directory and service address are known.

Linux, with a downloaded release asset:

```bash
sha256sum -c piping-desktop-linux-x86_64.AppImage.sha256
apps/desktop/scripts/install-linux.sh \
  --appimage piping-desktop-linux-x86_64.AppImage \
  --sha256 <sha256> \
  --service-url https://piping.example.com --direct
```

The script installs to `~/.local/opt/piping`, writes `app.json` under the
configuration directory above, and creates `piping.desktop`; use
`--no-desktop-entry` for image builds. It keeps an existing `app.json` and still
installs the binary; `--force-config` replaces the address file. To pre-seed a
machine image without running the script, create the file directly:

```json
{"serviceUrl":"https://piping.example.com","services":["https://piping.example.com"]}
```

Windows NSIS is silent with a service address:

```powershell
Piping_1.0.12_x64-setup.exe /S /SERVICE_URL=https://piping.example.com
```

For a per-machine image, write `%APPDATA%\piping\app.json` in each user profile
before the first launch. The installer seed next to the executable is
per-machine and only fills an empty configuration.

macOS: copy the application, then pre-create the configuration outside the
bundle:

```bash
cp -R Piping.app /Applications/
mkdir -p "$HOME/Library/Application Support/piping"
printf '%s\n' '{"serviceUrl":"https://piping.example.com","services":["https://piping.example.com"]}' \
  > "$HOME/Library/Application Support/piping/app.json"
```

Do not add files inside the signed `.app` bundle.

`--service-url`, `PIPING_DESKTOP_SERVICE_URL` and the `--direct` desktop entry
open the named instance without the chooser; without them the chooser opens as
usual. Configuration directories contain per-user WebView profiles: never share
one between users or machines, and treat its contents as secret. Use
`PIPING_DESKTOP_CONFIG_DIR` (absolute path) to place it elsewhere in a managed
image.

## Local development

```bash
cd apps/desktop
pnpm install
pnpm tauri dev
```

The dev build opens the local bootstrap UI. Enter the target deployed service,
for example `piping.example.com` or `http://localhost:9080`.

## Production build

Public GitHub Releases build native x86-64 Linux AppImage and Windows NSIS
installers, plus separate macOS DMGs for Apple Silicon and Intel. Assets use
`piping-desktop-<platform>.<extension>` within each versioned release; each has a `.sha256` checksum
and `.source.json` with the public commit, source commit and source pipeline.
The build uses the selected stable public tag, the pinned Rust toolchain and
the frozen desktop pnpm lockfile. Retrying refuses to replace an existing asset
with different bytes; a nondeterministic rebuild can therefore require manual
investigation rather than overwriting a published download.

The macOS DMGs require macOS 14 or newer and are ad-hoc signed, not notarized.
After copying Piping to
Applications, if macOS blocks it, open System Settings → Privacy & Security and
use Open Anyway for that app, then confirm the prompt. Do not disable Gatekeeper
globally. Unsigned Windows installers may show a SmartScreen warning. Building
the bundles does not establish runtime acceptance on those platforms.

Use the same source tree for all desktop clients. Build Linux artifacts on
Linux, macOS artifacts on macOS, and Windows NSIS installers on Windows. Windows
NSIS installers can also be cross-built from Linux via Tauri's supported
`cargo-xwin` MSVC flow.

Prerequisites:

- Node.js and pnpm versions from the workspace lockfile.
- The Rust toolchain pinned in the root `rust-toolchain.toml` and the Tauri platform toolchain.
- Linux: WebKitGTK, GStreamer, AppIndicator, librsvg, and standard build tools
  required by Tauri 2.
- macOS 14 or newer: Xcode Command Line Tools. macOS 14 is required for distinct
  persistent WebView stores per instance. Public bundles use ad-hoc signatures,
  not Developer ID or notarization; see [signing and installation](SIGNING.md).
- Windows native: Microsoft C++ Build Tools and WebView2 Runtime.
- Windows cross-build from Linux: NSIS, LLVM/LLD, `cargo-xwin`, and the
  `x86_64-pc-windows-msvc` Rust target.

Install dependencies once from the repository root or from `apps/desktop`:

```bash
pnpm install
```

Build every bundle supported by the current OS:

```bash
cd apps/desktop
pnpm tauri build
```

To build only the Linux AppImage:

```bash
cd apps/desktop
pnpm run build:appimage
```

The AppImage intentionally does not bundle the host graphics stack
(Wayland/GTK/WebKit libraries). Those libraries are tied to the desktop session,
GPU drivers, and distribution ABI, so the client uses the system copies from the
host. The AppImage launcher checks for the required system libraries and prints a
clear install hint if they are missing. Do not enable Tauri's
`bundleMediaFramework` option unless the Linux packaging strategy is revisited.

Required runtime packages:

- Debian/Ubuntu: `libwebkit2gtk-4.1-0`, `libgtk-3-0`
- Fedora/RHEL: `webkit2gtk4.1`, `gtk3`

The `.deb` and `.rpm` bundles declare these dependencies for the package manager.
The Linux release is built on Ubuntu 22.04; treat Ubuntu 22.04, Debian 12, and
Fedora releases with WebKitGTK 4.1 as the supported baseline. Older
distributions that only ship WebKitGTK 4.0/libsoup2 are not supported by the
AppImage.

The AppImage runtime may also need `libfuse2` on hosts that do not support
direct AppImage mounting. Users can still run it with
`APPIMAGE_EXTRACT_AND_RUN=1 ./Piping_*.AppImage` as a fallback. If a specific
GPU/Wayland setup opens a blank WebKit window, try
`WEBKIT_DISABLE_DMABUF_RENDERER=1` for diagnosis; do not set it globally unless
that host needs the workaround.

To build the Windows NSIS installer from Linux:

```bash
cd apps/desktop
cargo install --locked cargo-xwin
rustup target add x86_64-pc-windows-msvc
pnpm run build:windows
```

The Windows installer embeds the Microsoft Edge WebView2 Evergreen offline
installer. During setup Tauri checks whether WebView2 Runtime 110.0.1531.0 or
newer is already available and only runs the embedded installer when the runtime
is missing or older.

Piping Desktop supports Windows 10, Windows 11, and Windows Server 2016 or
newer. Windows Server 2012 and 2012 R2 are not supported because Microsoft Edge
WebView2 Runtime is limited to version 109 on those systems. The NSIS installer
stops early with a clear unsupported-OS message instead of letting WebView2 or
the app crash later.

For unattended Windows rollout, use NSIS silent mode and pass the service URL:

```powershell
.\Piping_0.1.0_x64-setup.exe /S /SERVICE_URL=https://piping.example.com
```

The installer writes `desktop-service-url.txt` next to the installed executable.
On first launch, Piping Desktop normalizes that value and stores it in the user's
normal `app.json`; the chooser then opens with the address preselected. The same
seed file works for per-machine installs because each Windows user gets their own
runtime config on first launch.

The desktop app also accepts runtime overrides when launching the executable
directly:

```powershell
Piping.exe --service-url https://piping.example.com
Piping.exe --piping-service-url=https://piping.example.com
```

For managed launchers that prefer environment variables:

```powershell
$env:PIPING_DESKTOP_SERVICE_URL = "https://piping.example.com"
.\Piping.exe
```

Command-line and environment overrides take precedence over the saved config.
The installer aliases `/PIPING_SERVICE_URL=` and `/SERVICEURL=` are accepted for
deployment systems that avoid underscores or reserve short option names.

Common outputs:

- macOS: `src-tauri/target/release/bundle/dmg/*.dmg` and `.app`.
- Windows native: `src-tauri/target/release/bundle/nsis/*.exe`.
- Windows cross-build: `src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/*-setup.exe`.
- Linux: `src-tauri/target/release/bundle/{appimage,deb,rpm}/`.

The Cargo target directory defaults to `src-tauri/target`; set
`CARGO_TARGET_DIR` to redirect it. The Linux AppImage is written to:

```text
<target-dir>/release/bundle/appimage/Piping_0.1.0_amd64.AppImage
```

For a faster local packaging smoke test:

```bash
cd apps/desktop
pnpm tauri build --debug
```

Smoke-test an AppImage without installing it:

```bash
chmod +x <target-dir>/release/bundle/appimage/Piping_0.1.0_amd64.AppImage
<target-dir>/release/bundle/appimage/Piping_0.1.0_amd64.AppImage
```

On first launch, enter a deployed Piping address such as
`piping.example.com`. Hostnames without a scheme use HTTPS automatically. Use
the app menu under the Piping logo to change the stored address later.

## Native commands

The trusted local bootstrap window and the configured service windows have
different capabilities, so they can call different commands.

Bootstrap window:

- `get_app_version`
- `get_service_url`
- `get_system_language`
- `set_service_url`
- `change_service_address`
- `show_about`
- `check_for_updates`
- `minimize_window`
- `toggle_maximize_window`
- `close_window`
- `start_window_drag`
- `show_app_menu`

Configured service windows (`main`, `detached-*`):

- `minimize_window`
- `toggle_maximize_window`
- `close_window`
- `start_window_drag`
- `show_app_menu`
- `open_detached_window`
- `open_external_url`
- `open_app_uri`

Both capabilities also grant the core window permissions that start a window
drag and a resize drag.

The permissions are generated by Tauri into
`src-tauri/permissions/autogenerated/` and granted per capability:
`src-tauri/capabilities/bootstrap.json` for the local bootstrap window and
`src-tauri/capabilities/service-windows.json` for the configured service
windows (`main`, `detached-*`).

GitLab owns desktop packaging and artifact publication; GitHub contains only
release snapshots of the source. Desktop packaging jobs are not yet part of
`.gitlab-ci.yml`, so the current build gate remains a local `pnpm tauri build`.
Platform runners and the signing setup below are required before those GitLab
jobs can publish installers. GitHub Actions does not build desktop bundles.

Do not add filesystem, shell-execute, store, or secret-bearing permissions to
remote service windows. The URL is user-configurable, so only minimal window
chrome commands belong there.

## Code signing

| OS | Required for distribution | Notes |
| --- | --- | --- |
| macOS | Apple Developer ID Application + Installer | Required for public distribution because Gatekeeper blocks unsigned apps |
| Windows | Optional but strongly recommended | Unsigned installers show SmartScreen warnings; EV certificate reduces friction |
| Linux | Optional | AppImage and deb can ship unsigned for internal preview builds |

For an internal tech preview, unsigned builds are acceptable. Public release
should add signing and notarization.

## Auto-updater

The updater plugin is included but disabled in `tauri.conf.json`.

To enable it later:

1. Generate a signing key with `pnpm tauri signer generate`.
2. Set `plugins.updater.pubkey`.
3. Add release manifest URLs to `plugins.updater.endpoints`.
4. Publish signed installer artifacts and `latest.json` from CI.

## Open items

1. Verify real SSO/OIDC flows in the OS WebViews used by each platform.
2. Add cross-platform run smoke for the multiwebview desktop shell.
