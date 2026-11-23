#!/usr/bin/env bash
# Builds dist/Rustcord.app from a release-profile build and reports binary sizes.
# Default: universal binary (arm64 + x86_64). `--native` builds only the host architecture.
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin ]]; then
  echo "This script must run on macOS." >&2
  exit 1
fi
case "${1:-}" in
  "") targets=(aarch64-apple-darwin x86_64-apple-darwin) ;;
  --native) targets=("$(uname -m | sed 's/^arm64$/aarch64/')-apple-darwin") ;;
  *) echo "usage: $0 [--native]" >&2; exit 2 ;;
esac
export MACOSX_DEPLOYMENT_TARGET=11.0
rustup target add "${targets[@]}"
slices=()
for target in "${targets[@]}"; do
  cargo build --release --locked --target "$target"
  slices+=("target/$target/release/rustcord")
done
app="dist/Rustcord.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
lipo -create -output "$app/Contents/MacOS/rustcord" "${slices[@]}"
chmod +x "$app/Contents/MacOS/rustcord"
lipo -info "$app/Contents/MacOS/rustcord"
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
</dict></plist>
PLIST
plutil -lint "$app/Contents/Info.plist"

size_row() {
  local bytes
  bytes=$(stat -f%z "$2")
  printf '| %s | %s | %s MiB |\n' "$1" "$bytes" "$(awk -v b="$bytes" 'BEGIN { printf "%.2f", b / 1048576 }')"
}
report=$(
  echo "### Release binary size"
  echo
  echo "| Binary | Bytes | Size |"
  echo "|---|---:|---:|"
  for i in "${!targets[@]}"; do size_row "${targets[$i]}" "${slices[$i]}"; done
  size_row "Rustcord.app/Contents/MacOS/rustcord" "$app/Contents/MacOS/rustcord"
)
echo "$report"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  echo "$report" >> "$GITHUB_STEP_SUMMARY"
fi
echo "Built $app"
