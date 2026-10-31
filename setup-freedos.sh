#!/bin/bash
# Setup FreeDOS 1.4 LiveCD ISO for installation into a hard-disk image.
# No root privileges required.

set -e

FREEDOS_ZIP="FD14-LiveCD.zip"
FREEDOS_ISO="FD14LIVE.iso"
FREEDOS_URL="https://download.freedos.org/1.4/FD14-LiveCD.zip"

echo "Setting up FreeDOS..."

if [ ! -f "$FREEDOS_ISO" ]; then
    if [ ! -f "$FREEDOS_ZIP" ]; then
        echo "Downloading FreeDOS 1.4 LiveCD (~294 MB)..."
        wget -O "$FREEDOS_ZIP" "$FREEDOS_URL"
    fi
    echo "Extracting ISO..."
    unzip -o "$FREEDOS_ZIP" FD14LIVE.iso
fi

if [ ! -f "$FREEDOS_ISO" ]; then
    echo "ERROR: $FREEDOS_ISO not found after extraction."
    exit 1
fi

echo "FreeDOS ISO ready: $FREEDOS_ISO"