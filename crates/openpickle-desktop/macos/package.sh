#!/bin/bash
# Builds OpenPickle.app and a disk image of it into target/macos/.
#
# Pass a Rust target (aarch64-apple-darwin or x86_64-apple-darwin) to cross-build it.
# Without an argument, the host target is used.
#
# The app is signed ad hoc. Another Mac may require an explicit first-launch approval because
# the app is not signed with a Developer ID or notarized.
set -euo pipefail

cd "$(dirname "$0")/../../.."
macos=crates/openpickle-desktop/macos
version=$(cargo pkgid -p openpickle-desktop | sed 's/.*[#@]//')
out=target/macos
app=$out/OpenPickle.app

target=${1:-}
if [[ -z "$target" ]]; then
    target=$(rustc -vV | sed -n 's/^host: //p')
    build_target=()
    binary=target/release/openpickle
else
    build_target=(--target "$target")
    binary=target/$target/release/openpickle
fi

case "$target" in
    aarch64-apple-darwin) arch=arm64 ;;
    x86_64-apple-darwin) arch=x86_64 ;;
    *)
        echo "unsupported macOS Rust target: $target" >&2
        exit 2
        ;;
esac
image=$out/OpenPickle-$version-$arch.dmg

cargo build --locked --release -p openpickle-desktop "${build_target[@]}"

# Keep images made for another architecture when this script is invoked twice in one job.
rm -rf "$app" "$out/AppIcon.iconset" "$out/image"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/openpickle"
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
