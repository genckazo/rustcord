#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin ]]; then
  echo "This script must run on macOS." >&2
  exit 1
fi
export MACOSX_DEPLOYMENT_TARGET=11.0
cargo build --release --locked
app="dist/Rustcord.app"
mkdir -p "$app/Contents/MacOS"
cp target/release/rustcord "$app/Contents/MacOS/rustcord"
chmod +x "$app/Contents/MacOS/rustcord"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleExecutable</key><string>rustcord</string>
  <key>CFBundleIdentifier</key><string>app.rustcord.desktop</string>
  <key>CFBundleName</key><string>Rustcord</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>Rustcord uses your microphone to transmit audio during Discord voice calls. Audio is never recorded to disk.</string>
</dict></plist>
PLIST
plutil -lint "$app/Contents/Info.plist"
echo "Built $app"
