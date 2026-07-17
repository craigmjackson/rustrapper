# Rustrapper

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A hybrid BIOS/UEFI bootloader written entirely in Rust. Scans storage devices
and network adapters from a boot menu.

Produces legacy BIOS (MBR+stage2) binaries, x86_64 UEFI and ARM64 EFI applications,
ARM64 bare-metal binaries, and PCI expansion ROMs. The only non-Rust code is
a tiny 16-bit MBR and protected-mode entry stub in NASM (~1 KB total).

## Features

- **BIOS** — 16‑bit MBR + 32-bit Rust stage2: menu, PCI storage scan, e1000 MMIO DHCP + DNS lookup, PXE boot
- **x86_64 UEFI** — Pure Rust PE/COFF: SNP protocol, DHCP client, ARP resolve, DNS lookup, storage scan, PXE boot
- **UEFI option ROM** — PCI expansion ROM with direct e1000 MMIO driver (no UEFI protocols needed during DXE)
- **ARM64 UEFI** — Same Rust code compiled for `aarch64-unknown-uefi`
- **ARM64 bare‑metal** — No firmware: PL011 UART, PCI ECAM walk, AHCI probe, PXE boot
- **BIOS option ROM** — Legacy PCI expansion ROM from `rust_payload.bin` via `romwrap --bios`
- **ROM wrapper** — Rust CLI tool wraps PE/COFF into UEFI PCI option ROM (`--bios` for BIOS option ROM)
- **PXE Boot** — DHCP options 66/67, TFTP client with block size negotiation, executes PE/COFF/ELF32/ELF64/Multiboot

## Quick Start

```bash
# Install Rust targets
rustup target add x86_64-unknown-uefi aarch64-unknown-uefi aarch64-unknown-none

# Install system dependencies
# Arch:  pacman -S nasm edk2-ovmf qemu-system-x86 qemu-system-aarch64
# Debian: apt install nasm ovmf qemu-efi-aarch64 qemu-system-x86 qemu-system-arm
# Fedora: dnf install nasm edk2-ovmf edk2-aarch64 qemu-system-x86 qemu-system-arm

make all                          # Build everything
make run-x86_64-uefi              # x86_64 UEFI in QEMU (e1000 NIC, full DHCP, PXE boot)
make run-x86_64-uefi-rom          # x86_64 UEFI with custom option ROM + DHCP
make run-i386-bios                # Legacy BIOS with PXE boot support
```

The `run-*` targets automatically create a `tftp-root/` directory containing bootloader binaries and a test file. QEMU's built-in TFTP server serves these files when DHCP options 66/67 are present.

## Build Targets

| Target                 | Binary                                         | Description                         |
| ---------------------- | ---------------------------------------------- | ----------------------------------- |
| `make i386-bios`       | `bin/rust_payload.bin`, `bin/stage2_entry.bin` | 32‑bit BIOS stage2                  |
| `make x86_64-uefi`     | `bin/rustrapper.efi`                           | x86_64 UEFI application             |
| `make aarch64-uefi`    | `bin/rustrapper_arm64.efi`                     | ARM64 UEFI application              |
| `make aarch64-bare`    | `bin/rustrapper_arm64_bare.elf`                | ARM64 bare‑metal                    |
| `make x86_64-uefi-rom` | `bin/rustrapper_efi.rom`                       | PCI expansion ROM (UEFI option ROM) |
| `make i386-bios-rom`   | `bin/rustrapper_bios.rom`                      | PCI expansion ROM (BIOS option ROM) |

## Run in QEMU

```bash
make run-i386-bios                # Legacy BIOS stage2 (serial, Ctrl-A X to exit)
make run-i386-bios-rom            # Legacy BIOS with PCI expansion ROM (rust_payload.bin)
make run-x86_64-uefi              # x86_64 UEFI (e1000 NIC, SNP protocol, full DHCP)
make run-x86_64-uefi-rom          # x86_64 UEFI with custom option ROM (direct e1000 MMIO)
make run-aarch64-uefi             # ARM64 UEFI (virtio-net-pci NIC, DHCP OFFER)
make run-aarch64-bare             # ARM64 bare‑metal with AHCI drive
```

All BIOS targets use `-nographic` (Ctrl-A X to exit). The BIOS stage2 target (`run-i386-bios`) writes to both serial and VGA, so it also works with `-display curses` if you want a graphical view (Ctrl-A X won't work in curses mode — kill the process instead).

## Network Support

| Target                   | NIC            | Method                          | DHCP                                      | PXE                                      |
| ------------------------ | -------------- | ------------------------------- | ----------------------------------------- | ----------------------------------------- |
| BIOS Rust stage2 (disk)  | e1000          | PCI I/O ports + MMIO            | Full DHCP (DISCOVER→OFFER)                | TFTP download + execute (ELF32/Multiboot) |
| BIOS (option ROM)        | e1000          | PCI option ROM with PCIR header | e1000 I/O BAR driver (real hardware only) | TFTP download + execute (ELF32/Multiboot) |
| x86_64 UEFI (disk)       | e1000          | SNP protocol                    | Full DHCP (DISCOVER/OFFER/REQUEST/ACK)    | TFTP download + execute (PE/COFF)         |
| x86_64 UEFI (option ROM) | e1000          | Direct MMIO + I/O port PCI scan | Full DHCP (DISCOVER→OFFER)                | TFTP download + execute (PE/COFF)         |
| ARM64 UEFI               | virtio-net-pci | SNP protocol                    | Single-transmit DHCP (DISCOVER→OFFER)     | TFTP download + execute (PE/COFF)         |
| ARM64 bare-metal         | e1000          | PCI ECAM + MMIO                 | Full DHCP (DISCOVER→OFFER)                | TFTP download + execute (ELF64)           |

## PXE Boot

All targets support PXE boot via DHCP options 66 (TFTP server) and 67 (bootfile name). When these options are present in the DHCP response, the bootloader automatically:

1. Downloads the specified file via TFTP (RFC 1350 with RFC 2348 block size negotiation)
2. Detects the file format by magic number (PE/COFF, ELF32, ELF64, Multiboot, Multiboot2, text, or binary)
3. Executes the file if it's a recognized executable format, or displays it if it's text

QEMU's built-in TFTP server (`-netdev user,tftp=...,bootfile=...`) provides PXE services without requiring root privileges or external servers. The `tftp-root/` directory is automatically populated with bootloader binaries and a test file.

## Project Structure

```
├── common/             # no_std Rust library (print, scan, menu, e1000, dhcp, arp, dns, netio, tftp, loader)
│   └── src/
│       ├── menu.rs     # Shared [1]/[2] menu logic
│       ├── print.rs    # Callback-based print (putc/puts/print_hex/print_dec/print_ip)
│       ├── scan.rs     # Generic device-scan loop
│       ├── e1000.rs    # Direct MMIO e1000 driver (init/send/recv) — shared by all targets
│       ├── arp.rs      # ARP request build + reply parse
│       ├── dns.rs      # DNS query build + response parse + unicast UDP frame build
│       ├── netio.rs    # e1000 glue: ARP resolve + DNS lookup + dns_resolve_and_print
│       ├── dhcp.rs     # DHCP frame build/parse, IP checksum, DhcpConfig (incl. DNS server, PXE options)
│       ├── tftp.rs     # TFTP client (RFC 1350) with block size negotiation (RFC 2348), streaming transfer
│       └── loader.rs   # File format detection (PE/COFF, ELF32, ELF64, Multiboot, text, binary)
├── bios/                 # Rust 32-bit BIOS stage2
│   ├── Cargo.toml
│   ├── link.ld         # Link at 0x100000
│   ├── targets/i386-unknown-none.json
│   └── src/
│       ├── mbr.asm     # 512‑byte MBR stage‑1
│       ├── stage2_entry.nasm # Entry stub for Rust stage2 (A20, protected mode, payload copy)
│       ├── main.rs     # _start entry, menu dispatch
│       ├── serial.rs   # COM1 driver (putc, getc, flush)
│       ├── vga.rs      # VGA text-mode driver with scrolling
│       ├── pci.rs      # PCI scan via I/O ports 0xCF8/0xCFC
│       ├── net.rs      # PCI + e1000 scan, DHCP, PXE boot (thin wrapper over common)
│       ├── mem.rs      # Extended memory allocation via INT 15h E820
│       └── loader.rs   # ELF32/Multiboot execution
├── uefi/               # Rust UEFI binary (x86_64 + ARM64)
│   └── src/
│       ├── efi.rs      # Hand-typed EFI types, GUIDs, function offsets
│       ├── scan.rs     # Storage device enumeration
│       ├── net.rs      # SNP + direct e1000 DHCP client, PXE boot (uses common e1000/dhcp/tftp)
│       ├── mem.rs      # UEFI memory allocation (AllocatePool/FreePool)
│       ├── loader.rs   # PE/COFF execution (LoadImage/StartImage)
│       └── main.rs     # efi_main entry point
├── arm64-bare/         # Rust ARM64 bare‑metal binary
│   └── src/
│       ├── pci.rs      # PCI ECAM walk, BAR sizing, AHCI probe
│       ├── uart.rs     # PL011 UART driver
│       ├── net.rs      # PCI + e1000 scan, DHCP, PXE boot (thin wrapper over common)
│       ├── mem.rs      # Fixed RAM region allocation
│       ├── loader.rs   # ELF64 execution
│       └── main.rs     # global_asm! entry, UART/PCI init
├── romwrap/            # CLI tool: wraps PE/COFF into PCI option ROM
├── tftp-root/          # Files served via QEMU's built-in TFTP server (auto-generated)
├── Makefile            # Build orchestration
└── AGENTS.md           # Full development reference & gotchas
```

## Tests

All crates are host‑testable — platform‑specific code is guarded with `#[cfg(not(test))]`.

```bash
cargo test --workspace   # 174 tests across all crates
```

| Crate        | Tests | What's Tested                                                                                |
| ------------ | ----- | -------------------------------------------------------------------------------------------- |
| `common`     | 96    | Hex/decimal formatting, device info, scan loop with mocks, DHCP build/parse (incl. PXE options), ARP build/parse, DNS build/parse, subnet check, TFTP protocol, file format detection |
| `uefi`       | 24    | EFI type sizes, GUID values, SNP mode layout, constants, PCI IO protocol                     |
| `arm64-bare` | 21    | PCI offset encoding, storage subclass naming                                                 |
| `romwrap`    | 33    | PCIR layout, BIOS/UEFI code types, entry routine, 512-byte alignment, edge cases             |

## Requirements

- **Rust** with targets: `x86_64-unknown-uefi`, `aarch64-unknown-uefi`, `aarch64-unknown-none`, `i386-bios` (needs `-Zjson-target-spec` and `-Zbuild-std=core`)
- **BIOS**: `nasm` and `objcopy` (for assembling the MBR/entry stub and stripping the ELF to a flat binary)
- **Testing**: `qemu-system-x86_64` (with OVMF), `qemu-system-aarch64` (with `QEMU_EFI.fd`)
