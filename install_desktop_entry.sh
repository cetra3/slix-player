#!/bin/sh
set -e

DIR="$(cd "$(dirname "$0")" && pwd)"
PREFIX="${HOME}/.local"

install -Dm644 "${DIR}/slix-player.desktop" "${PREFIX}/share/applications/slix-player.desktop"
install -Dm644 "${DIR}/ui/icons/icon.svg" "${PREFIX}/share/icons/slix-player.svg"

echo "Installed desktop entry and icon to ${PREFIX}/share"
