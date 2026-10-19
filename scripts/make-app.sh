#!/bin/sh
# Build viewr.app — a regular macOS application bundle that Finder can open
# jpg/jpeg/png/NEF files with (see packaging/Info.plist for the file types).
#
#   scripts/make-app.sh            build and assemble dist/Viewr.app, ad-hoc signed
#   scripts/make-app.sh --install  also copy it to /Applications (or ~/Applications
#                                  when /Applications is not writable)
#
# The result is only ad-hoc signed: fine for your own machine. Distributing it to
# other Macs would need a Developer ID signature and notarization.
set -eu

cd "$(dirname "$0")/.."
INSTALL=${1:-}
LSREGISTER=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
[ -n "$VERSION" ] || { echo "make-app: could not read version from Cargo.toml" >&2; exit 1; }

echo "==> cargo build --release"
cargo build --release

APP=dist/Viewr.app
echo "==> assembling $APP (version $VERSION)"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/viewr "$APP/Contents/MacOS/viewr"
sed "s/__VERSION__/$VERSION/g" packaging/Info.plist > "$APP/Contents/Info.plist"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
printf 'APPL????' > "$APP/Contents/PkgInfo"

echo "==> icon"
ICONSET=$(mktemp -d)/Viewr.iconset
mkdir -p "$ICONSET"
# iconutil wants the full size ladder; sips derives each from the 1024 master.
for spec in "16 icon_16x16.png" "32 icon_16x16@2x.png" "32 icon_32x32.png" \
            "64 icon_32x32@2x.png" "128 icon_128x128.png" "256 icon_128x128@2x.png" \
            "256 icon_256x256.png" "512 icon_256x256@2x.png" "512 icon_512x512.png" \
            "1024 icon_512x512@2x.png"; do
    set -- $spec
    sips -z "$1" "$1" packaging/icon-1024.png --out "$ICONSET/$2" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/Viewr.icns"
rm -rf "$(dirname "$ICONSET")"

echo "==> ad-hoc signing"
codesign --force --deep --sign - "$APP"
codesign --verify --strict "$APP"

DEST=$APP
if [ "$INSTALL" = "--install" ]; then
    if [ -w /Applications ]; then DEST_DIR=/Applications; else DEST_DIR="$HOME/Applications"; fi
    mkdir -p "$DEST_DIR"
    DEST="$DEST_DIR/Viewr.app"
    echo "==> installing to $DEST"
    rm -rf "$DEST"
    cp -R "$APP" "$DEST"
fi

# Make (re)registering explicit so new file types show up in Finder promptly.
"$LSREGISTER" -f "$DEST" || true

case $DEST in
    /*) DEST_ABS=$DEST ;;
    *) DEST_ABS="$PWD/$DEST" ;;
esac

echo
echo "Built $DEST"
echo "Open an image with it from Finder (right-click > Open With > viewr), or:"
echo "  open -a \"$DEST_ABS\" /path/to/photo.jpg"
echo "To make it the default for a type: Finder > select a file > Get Info >"
echo "Open with: viewr > Change All…"
