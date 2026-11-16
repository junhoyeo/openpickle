#!/bin/bash
# Builds OpenPickle.app and a disk image of it into target/macos/.
#
# The app is signed ad hoc, which is enough to run it on the Mac that built it. Another Mac
# refuses it until it is signed with a Developer ID and notarized.
set -euo pipefail

cd "$(dirname "$0")/../../.."
macos=crates/openpickle-desktop/macos
version=$(cargo pkgid -p openpickle-desktop | sed 's/.*[#@]//')
out=target/macos
app=$out/OpenPickle.app
image=$out/OpenPickle-$version-$(uname -m).dmg

cargo build --release -p openpickle-desktop

rm -rf "$out"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/openpickle "$app/Contents/MacOS/openpickle"
sed "s/__VERSION__/$version/g" "$macos/Info.plist" > "$app/Contents/Info.plist"

# macOS wants the icon in ten sizes; all come from the one 1024 px picture.
iconset=$out/AppIcon.iconset
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$macos/AppIcon.png" --out "$iconset/icon_${size}x${size}.png" > /dev/null
    sips -z "$((size * 2))" "$((size * 2))" "$macos/AppIcon.png" --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$iconset"

codesign --force --sign - "$app"

# The image holds the app next to a shortcut to /Applications, for dragging one onto the other.
stage=$out/image
mkdir -p "$stage"
cp -R "$app" "$stage/"
ln -s /Applications "$stage/Applications"
hdiutil create -volname OpenPickle -srcfolder "$stage" -ov -format UDZO "$image" > /dev/null
rm -rf "$stage"

echo "$image"
