#!/usr/bin/env bash
set -euo pipefail
shopt -s nullglob

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
desktop_dir="$(cd -- "$script_dir/.." && pwd)"

if [ "$(uname -s)" != "Linux" ]; then
  echo "build-linux-bundles.sh must run on Linux." >&2
  exit 1
fi

cd "$desktop_dir"

if [ -z "${TMPDIR:-}" ]; then
  if [ -d /data ] && [ -w /data ]; then
    export TMPDIR=/data/tmp/piping-desktop-build
  else
    export TMPDIR="${RUNNER_TEMP:-/tmp}/piping-desktop-build"
  fi
fi
mkdir -p "$TMPDIR"
export TEMP="$TMPDIR"
export TMP="$TMPDIR"

cargo_target_dir="$(
  cargo metadata --manifest-path src-tauri/Cargo.toml --format-version=1 --no-deps \
    | node -e "let input=''; process.stdin.on('data', chunk => input += chunk); process.stdin.on('end', () => process.stdout.write(JSON.parse(input).target_directory));"
)"

bundle_csv=""
tauri_args=()
while [ "$#" -gt 0 ]; do
  case "$1" in
    --bundles | -b)
      shift
      bundle_csv="${1:-}"
      ;;
    --bundles=*)
      bundle_csv="${1#--bundles=}"
      ;;
    -b=*)
      bundle_csv="${1#-b=}"
      ;;
    *)
      tauri_args+=("$1")
      ;;
  esac
  shift || true
done

bundle_csv="${bundle_csv:-appimage,deb,rpm}"
bundle_csv="${bundle_csv// /,}"
wants_appimage=false
tauri_bundles=()
IFS=',' read -r -a requested_bundles <<< "$bundle_csv"
for bundle in "${requested_bundles[@]}"; do
  case "$bundle" in
    appimage)
      wants_appimage=true
      ;;
    deb | rpm)
      tauri_bundles+=("$bundle")
      ;;
    "")
      ;;
    *)
      echo "Unsupported Linux bundle target: $bundle" >&2
      exit 1
      ;;
  esac
done

if [ "${#tauri_bundles[@]}" -gt 0 ]; then
  tauri_bundle_csv="$(IFS=,; echo "${tauri_bundles[*]}")"
  pnpm exec tauri build --config src-tauri/tauri.conf.json --bundles "$tauri_bundle_csv" "${tauri_args[@]}"
fi

if [ "$wants_appimage" != true ]; then
  exit 0
fi

rm -rf \
  "src-tauri/target/release/bundle/appimage" \
  "$cargo_target_dir/release/bundle/appimage" \
  "$cargo_target_dir"/release/.fingerprint/piping-desktop-* \
  "$cargo_target_dir"/release/build/piping-desktop-* \
  "$cargo_target_dir"/release/deps/piping_desktop-* \
  "$cargo_target_dir"/release/piping-desktop \
  "$cargo_target_dir"/release/piping-desktop.d

pnpm exec tauri build --config src-tauri/tauri.conf.json --no-bundle "${tauri_args[@]}"

version="$(node -p "require('./package.json').version")"
arch="$(uname -m)"
case "$arch" in
  x86_64)
    appimage_arch=amd64
    tool_arch=x86_64
    ;;
  aarch64 | arm64)
    appimage_arch=arm64
    tool_arch=aarch64
    ;;
  *)
    appimage_arch="$arch"
    tool_arch="$arch"
    ;;
esac

appimage_dir="$cargo_target_dir/release/bundle/appimage"
app_dir="$appimage_dir/Piping.AppDir"
original_appimage="$appimage_dir/Piping_${version}_${appimage_arch}.AppImage"
rm -rf "$appimage_dir"
mkdir -p \
  "$app_dir/usr/bin" \
  "$app_dir/usr/share/icons/hicolor/32x32/apps" \
  "$app_dir/usr/share/icons/hicolor/64x64/apps" \
  "$app_dir/usr/share/icons/hicolor/128x128/apps" \
  "$app_dir/usr/share/icons/hicolor/256x256@2/apps" \
  "$app_dir/usr/share/icons/hicolor/512x512/apps"

install -m 755 "$cargo_target_dir/release/piping-desktop" "$app_dir/usr/bin/piping-desktop"
install -m 644 src-tauri/icons/32x32.png "$app_dir/usr/share/icons/hicolor/32x32/apps/piping-desktop.png"
install -m 644 src-tauri/icons/64x64.png "$app_dir/usr/share/icons/hicolor/64x64/apps/piping-desktop.png"
install -m 644 src-tauri/icons/128x128.png "$app_dir/usr/share/icons/hicolor/128x128/apps/piping-desktop.png"
install -m 644 src-tauri/icons/128x128@2x.png "$app_dir/usr/share/icons/hicolor/256x256@2/apps/piping-desktop.png"
install -m 644 src-tauri/icons/icon.png "$app_dir/usr/share/icons/hicolor/512x512/apps/piping-desktop.png"
install -m 644 src-tauri/icons/icon.png "$app_dir/Piping.png"
install -m 644 src-tauri/icons/icon.png "$app_dir/piping-desktop.png"
ln -sf piping-desktop.png "$app_dir/.DirIcon"

if [ ! -x "$app_dir/usr/bin/piping-desktop" ]; then
  echo "Linux AppDir is missing usr/bin/piping-desktop; cannot produce a runnable AppImage." >&2
  exit 1
fi

cat > "$app_dir/AppRun" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
this_dir="$(readlink -f "$(dirname "$0")")"
export APPDIR="${APPDIR:-$this_dir}"
export PATH="$this_dir/usr/bin:$PATH"

has_library() {
  local library="$1"

  if command -v ldconfig >/dev/null 2>&1 && ldconfig -p 2>/dev/null | grep -q -- "$library"; then
    return 0
  fi

  local directory
  for directory in /lib /lib64 /usr/lib /usr/lib64 /usr/local/lib; do
    if [ -d "$directory" ] && find "$directory" -name "$library" -print -quit 2>/dev/null | grep -q .; then
      return 0
    fi
  done

  return 1
}

missing=()
has_library "libwebkit2gtk-4.1.so.0" || missing+=("libwebkit2gtk-4.1.so.0")
has_library "libgtk-3.so.0" || missing+=("libgtk-3.so.0")

if [ "${#missing[@]}" -gt 0 ]; then
  message="Piping Desktop cannot start because required Linux desktop libraries are missing.

Missing:
$(printf '  - %s\n' "${missing[@]}")

Install the WebKitGTK 4.1 and GTK 3 runtime packages, then start Piping again.

Debian/Ubuntu:
  sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0

Fedora/RHEL:
  sudo dnf install webkit2gtk4.1 gtk3"

  if command -v zenity >/dev/null 2>&1; then
    zenity --error --title="Piping Desktop" --text="$message" || true
  elif command -v kdialog >/dev/null 2>&1; then
    kdialog --error "$message" --title "Piping Desktop" || true
  elif command -v xmessage >/dev/null 2>&1; then
    xmessage -title "Piping Desktop" "$message" || true
  fi

  printf '%s\n' "$message" >&2
  exit 127
fi

exec -a "$0" "$this_dir/usr/bin/piping-desktop" "$@"
SH
chmod +x "$app_dir/AppRun"

appstream_id="space.piping.Piping"
desktop_entry="$app_dir/$appstream_id.desktop"
rm -f "$app_dir"/*.desktop
cat > "$desktop_entry" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=Piping
Comment=Piping desktop client
Categories=Development;
Exec=piping-desktop
Icon=piping-desktop
StartupWMClass=piping-desktop
Terminal=false
DESKTOP
mkdir -p "$app_dir/usr/share/applications"
rm -f "$app_dir/usr/share/applications"/*.desktop
cp "$desktop_entry" "$app_dir/usr/share/applications/$appstream_id.desktop"

rm -rf "$app_dir/usr/share/metainfo"
mkdir -p "$app_dir/usr/share/metainfo"
cat > "$app_dir/usr/share/metainfo/$appstream_id.appdata.xml" <<'XML'
<?xml version="1.0" encoding="UTF-8"?>
<component type="desktop-application">
  <id>space.piping.Piping</id>
  <metadata_license>CC0-1.0</metadata_license>
  <project_license>LicenseRef-proprietary</project_license>
  <name>Piping</name>
  <summary>Desktop shell for the Piping remote-administration platform</summary>
  <developer id="space.piping">
    <name>Piping</name>
  </developer>
  <description>
    <p>Piping Desktop opens Piping workspaces and remote access sessions from a native desktop shell.</p>
  </description>
  <launchable type="desktop-id">space.piping.Piping.desktop</launchable>
  <content_rating type="oars-1.1" />
  <url type="homepage">https://piping.space</url>
</component>
XML

rm -rf \
  "$app_dir/apprun-hooks" \
  "$app_dir/lib" \
  "$app_dir/lib32" \
  "$app_dir/lib64" \
  "$app_dir/usr/lib" \
  "$app_dir/usr/lib32" \
  "$app_dir/usr/lib64" \
  "$app_dir/usr/share/doc" \
  "$app_dir/usr/share/glib-2.0"

forbidden="$(
  find "$app_dir" -type f \
    \( \
      -name 'libc.so*' -o \
      -name 'libstdc++*.so*' -o \
      -name 'libwayland*.so*' -o \
      -name 'libxkbcommon*.so*' -o \
      -name 'libEGL*.so*' -o \
      -name 'libGL*.so*' -o \
      -name 'libGLX*.so*' -o \
      -name 'libgbm*.so*' -o \
      -name 'libdrm*.so*' -o \
      -name 'libglib-2.0*.so*' -o \
      -name 'libgio-2.0*.so*' -o \
      -name 'libgobject-2.0*.so*' -o \
      -name 'libgmodule-2.0*.so*' -o \
      -name 'libgtk-*.so*' -o \
      -name 'libgdk-*.so*' -o \
      -name 'libgdk_pixbuf-*.so*' -o \
      -name 'libwebkit*.so*' -o \
      -name 'libjavascriptcoregtk*.so*' \
    \) \
    | sort
)"

if [ -n "$forbidden" ]; then
  {
    echo "Linux AppImage still bundles host graphics stack libraries:"
    echo "$forbidden"
  } >&2
  exit 1
fi

version="$(node -p "require('./package.json').version")"
arch="$(uname -m)"
tool="${APPIMAGETOOL:-}"
appimage_cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/piping-desktop-appimage"
mkdir -p "$appimage_cache_dir"

if [ -z "$tool" ]; then
  if command -v appimagetool >/dev/null 2>&1; then
    tool="$(command -v appimagetool)"
  else
    tool="$appimage_cache_dir/appimagetool-$tool_arch.AppImage"
    if [ ! -s "$tool" ]; then
      rm -f "$tool.tmp"
      curl --retry 3 --connect-timeout 15 --max-time 120 -fsSL \
        "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$tool_arch.AppImage" \
        -o "$tool.tmp"
      mv -f "$tool.tmp" "$tool"
    fi
    chmod +x "$tool"
  fi
fi

runtime_file="$appimage_cache_dir/runtime-$tool_arch"
if [ ! -s "$runtime_file" ]; then
  rm -f "$runtime_file.tmp"
  curl --retry 3 --connect-timeout 15 --max-time 120 -fsSL \
    "https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-$tool_arch" \
    -o "$runtime_file.tmp"
  mv -f "$runtime_file.tmp" "$runtime_file"
fi

rm -f "$original_appimage"
(
  cd "$appimage_dir"
  tool_args=(--no-appstream --runtime-file "$runtime_file" "$app_dir")
  if [[ "$tool" == *.AppImage ]]; then
    tool_args=(--appimage-extract-and-run "${tool_args[@]}")
  fi
  ARCH="$arch" LINUXDEPLOY_OUTPUT_VERSION="$version" "$tool" "${tool_args[@]}"
)

rebuilt_images=("$appimage_dir"/*.AppImage)
if [ ! -f "${rebuilt_images[0]:-}" ]; then
  echo "No AppImage was produced after pruning Linux graphics stack libraries." >&2
  exit 1
fi

mv -f "${rebuilt_images[0]}" "$original_appimage"
