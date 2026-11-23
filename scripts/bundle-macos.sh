#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin ]]; then
  echo "This script must run on macOS." >&2
  exit 1
fi
export MACOSX_DEPLOYMENT_TARGET=11.0
target="$(rustc -vV | sed -n 's/^host: //p')"
case "$target" in
  aarch64-apple-darwin) arch=arm64 ;;
  x86_64-apple-darwin) arch=x86_64 ;;
  *) echo "Unsupported Rust host: $target" >&2; exit 1 ;;
esac
cargo build --release --locked --target "$target" --target-dir target
package_id="$(cargo pkgid --locked -p rustcord)"
version="${package_id##*#}"
bundle_version="${version%%[-+]*}"

work="$(mktemp -d "${TMPDIR:-/tmp}/rustcord.XXXXXX")"
stage="$work/stage"
mount="$work/mount"
mounted=0
cleanup() {
  if (( mounted )); then
    hdiutil detach "$mount" >&2 || return 1
  fi
  rm -rf "$work"
}
trap cleanup EXIT

app="$stage/Rustcord.app"
executable="Contents/MacOS/rustcord"
dmg_name="Rustcord-${version}-macos-${arch}.dmg"
dmg="$work/$dmg_name"
mkdir -p "$app/Contents/MacOS"
install -m 755 "target/$target/release/rustcord" "$app/$executable"
ln -s /Applications "$stage/Applications"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>rustcord</string>
  <key>CFBundleIdentifier</key><string>app.rustcord.desktop</string>
  <key>CFBundleName</key><string>Rustcord</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$bundle_version</string>
  <key>CFBundleVersion</key><string>$bundle_version</string>
  <key>LSMinimumSystemVersion</key><string>$MACOSX_DEPLOYMENT_TARGET</string>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
plutil -lint "$app/Contents/Info.plist"
if [[ -n "${MACOS_SIGNING_IDENTITY:-}" ]]; then
  codesign --force --options runtime --timestamp --sign "$MACOS_SIGNING_IDENTITY" "$app"
else
  codesign --force --sign - "$app"
fi

hdiutil create -srcfolder "$stage" -volname Rustcord -format UDZO "$dmg"
if [[ -n "${MACOS_SIGNING_IDENTITY:-}" ]]; then
  codesign --timestamp --sign "$MACOS_SIGNING_IDENTITY" --identifier app.rustcord.desktop.dmg "$dmg"
  codesign --verify "$dmg"
fi
hdiutil verify "$dmg"

mkdir "$mount"
hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount"
mounted=1
packaged_app="$mount/Rustcord.app"
test "$(readlink "$mount/Applications")" = /Applications
test -x "$packaged_app/$executable"
lipo "$packaged_app/$executable" -verify_arch "$arch"
codesign --verify --strict "$packaged_app"
hdiutil detach "$mount"
mounted=0

mkdir -p dist
rm -rf dist/Rustcord.app
mv "$app" dist/Rustcord.app
mv -f "$dmg" "dist/$dmg_name"
echo "Built dist/Rustcord.app and dist/$dmg_name"
