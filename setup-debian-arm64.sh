#!/bin/bash
set -e
DEBIAN_ISO="debian-13.7.0-arm64-netinst.iso"
DEBIAN_URL="https://cdimage.debian.org/debian-cd/current/arm64/iso-cd/debian-13.7.0-arm64-netinst.iso"
echo "Setting up Debian 13 arm64 network installer..."
if [ ! -f "$DEBIAN_ISO" ]; then
    echo "Downloading Debian 13.7.0 arm64 netinst (~701 MB)..."
    wget -O "$DEBIAN_ISO" "$DEBIAN_URL"
fi
[ -f "$DEBIAN_ISO" ] || { echo "ERROR: $DEBIAN_ISO missing"; exit 1; }
echo "Debian arm64 netinst ISO ready: $DEBIAN_ISO"
