# piping-desktop - Tauri 2 shell

Desktop client for Piping on Windows, macOS, and Linux.

The app is intentionally a thin native shell around a deployed Piping web
origin. On first launch it opens a local bootstrap screen, asks for the service
address, stores it in the OS app config directory, and then opens the configured
remote service in the main WebView. Future launches go straight to that service.

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
- The custom top bar exposes update check, about, service address reset, and
  window controls.
- `window.open` requests from remote content are allowed only for the configured
  service origin and are opened as additional native desktop windows.

The stored config file is named `desktop-config.json` under Tauri's
`app_config_dir` for the bundle identifier `space.piping.desktop`.

## Local development

```bash
cd apps/desktop
pnpm install
pnpm tauri dev
```

The dev build opens the local bootstrap UI. Enter the target deployed service,
for example `piping.example.com` or `http://localhost:9080`.

## Production build

Use the same source tree for all desktop clients. Build Linux artifacts on
Linux, macOS artifacts on macOS, and Windows NSIS installers on Windows. Windows
NSIS installers can also be cross-built from Linux via Tauri's supported
`cargo-xwin` MSVC flow.

Prerequisites:

- Node.js and pnpm versions from the workspace lockfile.
- Rust stable and the Tauri platform toolchain.
- Linux: WebKitGTK, GStreamer, AppIndicator, librsvg, and standard build tools
  required by Tauri 2.
- macOS: Xcode Command Line Tools.
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
On first launch, Piping Desktop normalizes that value, stores it in the user's
normal `desktop-config.json`, and opens the configured service directly. The
same seed file works for per-machine installs because each Windows user gets
their own runtime config on first launch.

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
