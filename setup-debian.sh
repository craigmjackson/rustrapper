#!/bin/bash
# Setup Debian 13 (trixie) network installer ISO for UEFI validation.
# No root privileges required.

set -e

DEBIAN_ISO="debian-13.7.0-amd64-netinst.iso"
DEBIAN_URL="https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/debian-13.7.0-amd64-netinst.iso"

echo "Setting up Debian network installer..."

if [ ! -f "$DEBIAN_ISO" ]; then
    echo "Downloading Debian 13.7.0 netinst (~756 MB)..."
    wget -O "$DEBIAN_ISO" "$DEBIAN_URL"
fi

if [ ! -f "$DEBIAN_ISO" ]; then
    echo "ERROR: $DEBIAN_ISO not found after download."
    exit 1
fi

echo "Debian netinst ISO ready: $DEBIAN_ISO"
