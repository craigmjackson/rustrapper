#!/bin/bash
# Setup OpenWrt PXE server image
# This script downloads and configures OpenWrt for use as a PXE server
# No root privileges required

set -e

OPENWRT_VERSION="23.05.5"
OPENWRT_URL="https://downloads.openwrt.org/releases/${OPENWRT_VERSION}/targets/x86/64/openwrt-${OPENWRT_VERSION}-x86-64-generic-ext4-combined.img.gz"
OPENWRT_GZ="openwrt-${OPENWRT_VERSION}-x86-64-generic-ext4-combined.img.gz"
OPENWRT_IMG="openwrt"

# Partition 2 offset (from fdisk -l)
PART2_START_SECTOR=33792
PART2_NUM_SECTORS=212992
SECTOR_SIZE=512

echo "Setting up OpenWrt PXE server..."

# Download OpenWrt if not present
if [ ! -f "$OPENWRT_GZ" ]; then
    echo "Downloading OpenWrt ${OPENWRT_VERSION}..."
    wget -O "$OPENWRT_GZ" "$OPENWRT_URL"
fi

# Decompress if needed
if [ ! -f "$OPENWRT_IMG" ]; then
    echo "Decompressing OpenWrt image..."
    gunzip -k "$OPENWRT_GZ" || true
    if [ -f "openwrt-${OPENWRT_VERSION}-x86-64-generic-ext4-combined.img" ]; then
        mv "openwrt-${OPENWRT_VERSION}-x86-64-generic-ext4-combined.img" "$OPENWRT_IMG"
    fi
fi

# Check if we need to configure the image
if [ ! -f ".openwrt-configured" ]; then
    echo "Configuring OpenWrt image..."
    
    # Create temporary directory for config files
    CONFIG_DIR=$(mktemp -d)
    PART2_FILE=$(mktemp)
    
    # Create network configuration
    cat > "$CONFIG_DIR/network" << 'EOF'
config interface 'loopback'
    option device 'lo'
    option proto 'static'
    option ipaddr '127.0.0.1'
    option netmask '255.0.0.0'

config interface 'lan'
    option device 'eth0'
    option proto 'static'
    option ipaddr '10.0.0.1'
    option netmask '255.255.255.0'
EOF
    
    # Create DHCP configuration with PXE options
    cat > "$CONFIG_DIR/dhcp" << 'EOF'
config dnsmasq
    option domainneeded '1'
    option localise_queries '1'
    option authoritative '1'
    option leasefile '/tmp/dhcp.leases'
    option enable_tftp '1'
    option tftp_root '/tftpboot'
    option logdhcp '1'

config dhcp 'lan'
    option interface 'lan'
    option start '100'
    option limit '150'
    option leasetime '12h'
    option dhcpv4 'server'
    option dhcpv6 'disabled'
    option ra 'disabled'
    list dhcp_option '66,10.0.0.1'
    list dhcp_option '67,test.txt'
EOF
    
    # Extract partition 2
    echo "Extracting partition 2..."
    dd if="$OPENWRT_IMG" of="$PART2_FILE" bs=$SECTOR_SIZE skip=$PART2_START_SECTOR count=$PART2_NUM_SECTORS 2>/dev/null
    
    # Use debugfs to modify the ext4 filesystem
    echo "Creating config directories..."
    debugfs -w -R "mkdir /etc/config" "$PART2_FILE" 2>/dev/null || true
    
    echo "Writing network configuration..."
    debugfs -w -R "rm /etc/config/network" "$PART2_FILE" 2>/dev/null || true
    debugfs -w -R "write $CONFIG_DIR/network /etc/config/network" "$PART2_FILE"
    
    echo "Writing DHCP configuration..."
    debugfs -w -R "rm /etc/config/dhcp" "$PART2_FILE" 2>/dev/null || true
    debugfs -w -R "write $CONFIG_DIR/dhcp /etc/config/dhcp" "$PART2_FILE"
    
    echo "Creating TFTP root directory..."
    debugfs -w -R "mkdir /tftpboot" "$PART2_FILE" 2>/dev/null || true
    
    # Copy TFTP files if they exist
    if [ -d "tftp-root" ]; then
        echo "Copying TFTP files..."
        for file in tftp-root/*; do
            if [ -f "$file" ]; then
                filename=$(basename "$file")
                debugfs -w -R "rm /tftpboot/$filename" "$PART2_FILE" 2>/dev/null || true
                debugfs -w -R "write $file /tftpboot/$filename" "$PART2_FILE"
            fi
        done
    fi
    
    # Re-insert partition 2
    echo "Re-inserting partition 2..."
    dd if="$PART2_FILE" of="$OPENWRT_IMG" bs=$SECTOR_SIZE seek=$PART2_START_SECTOR count=$PART2_NUM_SECTORS conv=notrunc 2>/dev/null
    
    # Cleanup
    rm -rf "$CONFIG_DIR" "$PART2_FILE"
    
    # Mark as configured
    touch .openwrt-configured
    
    echo "OpenWrt configured successfully!"
else
    echo "OpenWrt already configured."
fi

echo "OpenWrt PXE server setup complete!"
echo "Image: $OPENWRT_IMG"
