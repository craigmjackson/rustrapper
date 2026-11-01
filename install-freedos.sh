#!/bin/bash
# Install FreeDOS 1.4 onto the FAT16 partition of a rustrapper disk image,
# entirely from the host (no QEMU, no interactive installer).
#
# The image must contain rustrapper's MBR + partition table (written by the
# Makefile's bin/bios.img rule): one active FAT16 partition starting at
# LBA 2048 — well past the stage2 load area (sectors 1..450), so the FreeDOS
# filesystem can never overwrite the bootloader. (An install into a partition
# starting at the old CHS default, sector 63, does overwrite it — the
# bootloader then triple-faults on the next boot.)
#
# Steps:
#   1. mformat the partition at its image offset, using the FreeDOS 1.4
#      FAT16 boot-sector code (freedos/bootsect.bin) via -B: mformat keeps
#      the boot code (including the "KERNEL  SYS" filename it looks for) and
#      rewrites only the BPB — geometry, size, and hidden sectors = the
#      partition start.
#   2. Copy KERNEL.SYS + COMMAND.COM from the LiveCD ISO into the filesystem,
#      plus a minimal CONFIG.SYS and AUTOEXEC.BAT. AUTOEXEC.BAT switches the
#      console to COM1 (`CTTY COM1`) so FreeDOS output and input work in the
#      -nographic QEMU runs (serial console), and prints a boot marker.
#
# Usage: ./install-freedos.sh [image] [iso]
set -e

IMG=${1:-bin/bios.img}
ISO=${2:-FD14LIVE.iso}
BOOTSECT=freedos/bootsect.bin

PART_LBA=2048          # partition start (must match the partition table)
HEADS=16               # BIOS geometry for the CHS fallback paths
SPT=63
CYLINDERS=127          # floor((131072 - PART_LBA) / (HEADS * SPT))

fail() { echo "ERROR: $*" >&2; exit 1; }

[ -f "$IMG" ]      || fail "$IMG not found (run 'make i386-bios' first)"
[ -f "$ISO" ]      || fail "$ISO not found (run 'make freedos-iso' first)"
[ -f "$BOOTSECT" ] || fail "$BOOTSECT not found"
for tool in mformat mcopy mdir bsdtar; do
    command -v "$tool" >/dev/null || fail "$tool is required (install mtools / libarchive)"
done

OFF=$((PART_LBA * 512))

echo "Formatting FAT16 partition at LBA $PART_LBA with FreeDOS boot sector..."
mformat -i "$IMG@@$OFF" -t "$CYLINDERS" -h "$HEADS" -n "$SPT" \
    -H "$PART_LBA" -v FREEDOS -B "$BOOTSECT" ::

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "Extracting FreeDOS system files from $ISO..."
bsdtar -xOf "$ISO" KERNEL.SYS  > "$TMP/KERNEL.SYS"
bsdtar -xOf "$ISO" COMMAND.COM > "$TMP/COMMAND.COM"
[ -s "$TMP/KERNEL.SYS" ]  || fail "KERNEL.SYS missing from $ISO"
[ -s "$TMP/COMMAND.COM" ] || fail "COMMAND.COM missing from $ISO"

# DOS line endings. CTTY COM1 makes FreeDOS talk on the serial console
# (all QEMU run targets use -nographic).
printf 'SHELL=COMMAND.COM /P\r\n' > "$TMP/CONFIG.SYS"
printf '@CTTY COM1\r\nECHO FreeDOS boot OK\r\nVER\r\n' > "$TMP/AUTOEXEC.BAT"

echo "Copying system files..."
mcopy -i "$IMG@@$OFF" -o \
    "$TMP/KERNEL.SYS" "$TMP/COMMAND.COM" "$TMP/CONFIG.SYS" "$TMP/AUTOEXEC.BAT" ::

echo "Installed:"
mdir -i "$IMG@@$OFF" ::
echo "Done. Boot with 'make run-i386-bios' and choose [4] Boot from hard disk."
