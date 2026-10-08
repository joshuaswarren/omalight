#!/bin/sh
# Install Omalight into PREFIX (default: ~/.local) for the current architecture.
# Arch x86_64 and aarch64 use the same recipe. The PKGBUILD is the package path.
set -eu
cd "$(dirname "$0")/.."
prefix="${PREFIX:-${HOME}/.local}"
echo "building release (locked) into ${prefix}"
cargo build --release --locked -p omalight
install -d "${prefix}/bin" \
  "${prefix}/share/applications" \
  "${prefix}/share/icons/hicolor/scalable/apps" \
  "${prefix}/share/omalight/skills/omalight"
install -m755 target/release/omalight target/release/omalight-engine target/release/omalight-mcp "${prefix}/bin/"
install -m644 packaging/omalight.desktop "${prefix}/share/applications/omalight.desktop"
install -m644 packaging/omalight.svg "${prefix}/share/icons/hicolor/scalable/apps/omalight.svg"
install -m644 skills/omalight/SKILL.md "${prefix}/share/omalight/skills/omalight/SKILL.md"
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "${prefix}/share/applications" >/dev/null 2>&1 || true
fi
"${prefix}/bin/omalight" skill --link
echo "installed ${prefix}/bin/omalight"
echo "start the engine with: omalight-engine install-service"
