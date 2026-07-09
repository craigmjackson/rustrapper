# Rustrapper Hybrid BIOS/UEFI Bootloader

## Overview

Produces a single `bootloader.combined` disk image bootable under legacy BIOS and x86_64 UEFI, plus standalone ARM64 EFI and ARM64 bare-metal binaries. On startup, all variants present a menu to choose between scanning storage devices or booting from network (DHCP).

## Layout (bootloader.combined, 64 MB disk image)

| LBA | Offset   | Content                                                             |
| --- | -------- | ------------------------------------------------------------------- |
| 0   | `0x000`  | MBR (`bios.bin`, 512 bytes) with partition table at `0x1BE`         |
| 1–14| `0x200`  | Stage-2 (`stage2.bin`, up to 7168 bytes), loaded by MBR to `0x1000` |
| 15+ | `0x1E00` | FAT32 ESP containing `EFI/BOOT/BOOTX64.EFI`                         |

## Directory Structure

```
.
├── Cargo.toml              # Workspace root (4 member crates)
├── Makefile                # Build orchestration (make all, make run-*)
├── AGENTS.md               # This file
├── bios/                   # Retained C/NASM sources
│   ├── mbr.asm             # 16-bit MBR stage-1
│   ├── stage2.c            # 16-bit stage-2 scanner
│   ├── div64.c             # 64-bit division helpers for -m16
│   ├── print.c / print.h   # Shared formatting (putc callback, hex, dec)
│   └── scan.c / scan.h     # Shared scan loop (calls arch detect_device)
├── common/                 # Rust no_std library
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── menu.rs         # Shared [1]/[2] menu logic
│       ├── print.rs        # Callback-based print (putc/puts/print_hex/print_dec)
│       └── scan.rs         # Generic device-scan loop
├── uefi/                   # Rust UEFI binary (x86_64 + ARM64)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # efi_main entry point
│       ├── efi.rs          # Hand-typed EFI types, GUIDs, Boot Services offsets
│       ├── scan.rs         # UEFI device scan (LocateHandleBuffer + OpenProtocol)
│       └── net.rs          # DHCP, SNP send/receive, IP/ARP parsing
├── arm64-bare/             # Rust ARM64 bare-metal binary (no firmware)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # Entry (global_asm! start), UART init, PCI scan
│       ├── pci.rs          # PCI ECAM walk, BAR sizing/enable, AHCI probe
│       ├── net.rs          # e1000 MMIO driver + DHCP client (no firmware)
│       └── uart.rs         # PL011 UART driver
└── disk-image/             # Rust CLI tool (disk image combiner)
    ├── Cargo.toml
    └── src/
        └── main.rs         # Assembles MBR + stage2 + FAT32 → combined image
```

## Files

### `uefi/src/efi.rs` — EFI type definitions

- All types hand-written (no `uefi` crate dependency).
- `extern "efiapi"` calling convention is the EFI ABI on both x86_64 and ARM64.
- Function pointers that dereference raw pointers (e.g. `output_string`) must be `unsafe extern "efiapi" fn(...)`, NOT `extern "efiapi" fn(...)`. Using safe `extern "efiapi" fn` causes "unnecessary `unsafe` block" warnings when wrapping calls.
- **Boot Services function offsets** (hardcoded, from GNU-EFI / EDK2 headers):
  - `0x48` — `FreePool`
  - `0x118` — `OpenProtocol`
  - `0x138` — `LocateHandleBuffer`
- GUIDs must be `static` (not `const`) to guarantee stable address semantics when taking references.
- `UINTN` = `u64` on both x86_64 and ARM64 (LP64).

### `uefi/src/main.rs` — UEFI entry point

- `efi_main` is `extern "efiapi" fn(image_handle, system_table) -> !`.
- Stores a global reference to the system table for console input.
- Presents the `[1]/[2]` menu via `common::menu::show_menu`, using `EFI_SIMPLE_TEXT_INPUT_PROTOCOL.ReadKeyStroke` for non-blocking input.
- Dispatches to `scan_storage_devices` or `net::scan_network_devices` based on the choice.

### `uefi/src/scan.rs` — UEFI storage scan

- Function pointers read from Boot Services table via `read_boot_svc_fn::<T>(gbs, offset)` which casts an arbitrary offset to the target function type.
- `w16(con_out, s)` — converts `&str` to a null-terminated `[u16; 256]` and calls `OutputString`. The null terminator must be written explicitly at `buf[len]`; the buffer is NOT null-initialized because we only zero the first `len+1` entries via the write loop.
- `put_dec(con_out, val)` — converts `u64` to decimal digits using two temporary `[u16; 24]` arrays. Digits are written reversed into `rev[]`, then copied forward into `buf[]` starting at index 0 with an explicit null terminator `buf[j] = 0`. **Must null-terminate**: passing a mid-array pointer without a trailing null causes `OutputString` to read past the end of the buffer into stack garbage.
- `LocateHandleBuffer` with search type `2` (`ByProtocol`) enumerates all handles supporting `EFI_BLOCK_IO_PROTOCOL`.
- Each handle is opened with `OpenProtocol` for both Block IO and Device Path protocols using `EFI_OPEN_PROTOCOL_BY_HANDLE_PROTOCOL` (`0x00000001`).
- `EFI_DEVICE_PATH_PROTOCOL` type/subtype gives a hint about the device; the first node's type/subtype is read, not the full path.
- The handle buffer is freed with `FreePool` after enumeration.

### `uefi/src/net.rs` — UEFI network scan and DHCP

- `print_network_info` uses `LocateHandleBuffer` with `SNP_GUID` to find NICs, opens via `OpenProtocol` with `EFI_OPEN_PROTOCOL_GET_PROTOCOL` (`0x00000002`).
- SNP driver start: accepts both `EFI_SUCCESS`, `EFI_ALREADY_STARTED`, and the ARM64 firmware's error-bit-encoded variant (`0x8000000000000014`). ARM64 virtio SNP returns the latter.
- SNP initialize: failure is non-fatal. ARM64 virtio SNP returns `EFI_UNSUPPORTED` (with error bit).
- `dhcp_run` sends a single DHCPDISCOVER and waits for an OFFER — the ARM64 virtio SNP only supports one transmit per session (no buffer recycling without `Initialize`). x86_64 e1000 works normally with full DISCOVER→OFFER→REQUEST→ACK.
- `send_udp_dhcp` builds Ethernet/IP/UDP/DHCP frame from scratch and calls `SNP.Transmit`.
- `try_receive` calls `SNP.Receive` with non-null `SrcAddr`, `DestAddr`, and `Protocol` output pointers; some firmware requires them.
- `parse_dhcp_response` validates IP/UDP headers, DHCP magic cookie, transaction ID, MAC match, and extracts IP/subnet/gateway from DHCP options.

### `arm64-bare/src/main.rs` — ARM64 bare-metal entry

- Entry point via `global_asm!`: sets stack pointer, enables FP/SIMD access via `CPACR_EL1`, zeroes BSS, calls `main`.
- **FP/SIMD must be enabled before any Rust code runs**: `CPACR_EL1.FPEN` bits (20-21) default to trapping. rustc's compiler-generated `memset`/`memcpy` intrinsics use NEON instructions for buffers over a few hundred bytes; without `orr x9, x9, #(3 << 20)` / `msr cpacr_el1, x9` / `isb` in the boot stub, any such call takes a synchronous exception. Since no exception vector table is installed (`VBAR_EL1` defaults to 0), the CPU silently loops forever fetching garbage at address `0x200` (the "current EL, SPx, synchronous" vector offset) — this looks exactly like a hang with no error output.
- UART output via MMIO PL011 at `0x09000000` (QEMU virt machine).
- PCI ECAM at `0x4010_0000_00` (QEMU virt v11+).
- Load address: `0x4020_0000` (above DTB at `0x4000_0000`).
- Presents the `[1]/[2]` menu via `common::menu::show_menu`, reading from the PL011 UART (`uart::getc`), and dispatches to `scan::scan_devices` or `net::scan_network` based on the choice.

### `arm64-bare/src/net.rs` — ARM64 bare-metal e1000 NIC + DHCP

- Direct MMIO driver for the Intel 82540EM (`e1000`) emulated NIC — no firmware/UEFI involved, unlike the UEFI DHCP path in `uefi/src/net.rs`.
- Descriptor rings (`RxDescs`/`TxDescs`) are `#[repr(align(16))]` wrapper structs around `#[repr(C, packed)]` descriptor arrays; QEMU's e1000 requires 16-byte-aligned ring base addresses for `RDBAL`/`TDBAL`.
- **`RDLEN`/`TDLEN` minimum is 128 bytes** on QEMU's e1000 model — rings smaller than 8 descriptors (16 bytes each) are silently rejected (register reads back as the last valid value, not what was written).
- **`RDT` must be set to `NUM_RX_DESC - 1` after init**, not `0`. `RDH == RDT` means zero descriptors are owned by hardware (empty ring) — a very easy off-by-one that results in silently receiving nothing.
- All RX descriptors point at a single shared `RX_BUF` (2048 bytes); fine for the single in-flight DHCP transaction this driver performs, but would corrupt data under concurrent multi-packet traffic.
- DHCP RX polling must be RAM-only status checks (`RX_DESCS.0[idx].status & RX_STATUS_DD`), not MMIO register polling (`reg_read32(base, REG_RDH)`) in a tight loop — repeated MMIO reads under QEMU TCG are drastically slower than cached RAM reads and can make a poll loop take far longer than intended for the same iteration count.
- The RX poll loop needs roughly 100 million iterations (~1 second of wall time) for QEMU's slirp (`-nic user`) DHCP server to respond; smaller counts (e.g. 2 million, ~0.1s) reliably miss the OFFER even though the packet is delivered on the wire (verified with `-object filter-dump,netdev=net0,file=...` producing a valid pcap showing both DISCOVER and OFFER).
- Uses `-nic user,model=e1000` in `run-bare-arm64` (not `-net none`); QEMU's e1000 emulation works normally with full ARP/DHCP over user-mode (slirp) networking on both x86_64 and aarch64 hosts.

### `arm64-bare/src/pci.rs` — PCI/AHCI probe

- Walks PCI bus 0, enumerates all devices.
- For each mass-storage class device, calls `enable_bars` (BAR sizing via all-1s write, MMIO from `0x3E00_0000`) and `probe_ahci`.
- `probe_ahci` enables HBA (`GHC.AE`, bit 31 of ABAR+0x04), reads CAP/PI/SSTS to detect attached drives.
- `pci_read_config` / `pci_write_config`: MMIO ECAM access via pointer dereference.

### `common/src/menu.rs` — Shared menu logic

- `MenuAction` enum: `StorageScan` / `NetworkBoot`.
- `show_menu(puts, putc, get_key)` prints `[1]/[2]` and polls `get_key` until the user presses a valid choice.
- `puts`/`putc` are function pointers, so the same function can be used with ASCII (UART/BIOS) and UEFI (wrapped to u16 `OutputString`) output.

### `bios/stage2.c` — BIOS stage-2 menu and scan

- Presents `[1]/[2]` menu and reads the choice from the serial port (COM1, 0x3F8), because the BIOS stage-2 uses serial output. This matches the `-serial stdio` setup used by `run-bios` and `run-bios-pxe`.
- Dispatches to `scan_devices()` (INT 13h) or `pxe_scan()` (e1000 direct I/O BAR + PXE/UNDI fallback).

### `common/src/print.rs` — Shared formatting

- Callback-based: `print_init(putc_fn)` sets the output function.
- `print_hex(val, nibbles)` — prints hex, no leading zeros except for `0`.
- `print_dec(val)` — converts `u64` to decimal using a right-to-left buffer.
- All functions call the `putc` callback for each character.

### `common/src/scan.rs` — Shared device scan loop

- Generic loop: for `index` in `0..MAX_DEVICES`, calls an arch-specific `detect_device` callback, prints results.
- Architecture-specific detection is provided by `detect_device` via a function pointer or trait (not yet implemented — the current C stage2 and Rust UEFI/ARM64 targets each handle scanning independently).

### `disk-image/src/main.rs` — Disk image builder

- Takes `bios.bin` + `stage2.bin` + `rustrapper.efi` → `bootloader.combined`.
- Calls `mkfs.fat -F32`, `mmd`, `mcopy` (mtools) externally via `std::process::Command`.
- Writes MBR partition entry (type `0x0C` FAT32 LBA) at offset `0x1BE`.
- Partition table entry format (16 bytes): `<status><chs_start[3]><type><chs_end[3]><lba_start[4]><sector_count[4]>`.

### `Makefile` — Build orchestration

- Top-level targets: `all`, `bios`, `uefi`, `arm64`, `bare-arm64`, `combined`, `run-*`, `clean`.
- Uses `CARGO_TARGET_DIR=target` and per-target `RUSTFLAGS` for UEFI (needs `/NODEFAULTLIB` on x86_64).
- BIOS targets compile from `bios/` using the same GCC+NASM flags as the original project.
- mtools (`mmd`, `mcopy`) and `mkfs.fat` must be on `$PATH` for `make combined`.

## Key Gotchas

### Rust-specific

1. **EFI function pointer safety**: UEFI protocol function pointers that dereference raw pointers must be typed as `unsafe extern "efiapi" fn(...)`. Marking them safe triggers "unnecessary `unsafe` block" when wrapping the call.
2. **Null-terminate u16 arrays for OutputString**: `SIMPLE_TEXT_OUTPUT_PROTOCOL.OutputString` reads until a `0x0000` word. Always write an explicit `buf[j] = 0` after the last character. Passing a pointer into the middle of an array without a trailing null reads past the buffer into stack garbage.
3. **GUIDs must be `static`**, not `const`. Taking a reference to a `const` generates a new temporary each time, producing non-identical addresses. EFI runtime code compares GUID pointer addresses.
4. **`extern "efiapi"` is portable**: Both x86_64 and ARM64 UEFI use the same `extern "efiapi"` calling convention in Rust. No `ms_abi` distinction needed (unlike C where `__attribute__((ms_abi))` is required on x86_64).
5. **`/NODEFAULTLIB` on x86_64-unknown-uefi**: Without `RUSTFLAGS="-C link-args=/NODEFAULTLIB"`, the linker pulls in CRT startup which conflicts with the UEFI environment. ARM64 doesn't need this.
6. **`panic = "abort"`**: UEFI and bare-metal targets must set `panic = "abort"` in `Cargo.toml` (in `[profile.release]` or `[profile.dev]`). Panic unwinding is not supported.
7. **`aarch64-unknown-none` requires `global_asm!` entry**: There's no CRT; use `core::arch::global_asm!` to define the `_start` symbol that sets SP and clears BSS before calling Rust code.
8. **Enable `CPACR_EL1` FP/SIMD access before any Rust code runs on `aarch64-unknown-none`**: rustc's `memset`/`memcpy` compiler intrinsics use NEON registers for larger buffers (e.g. a 1514-byte Ethernet frame array). Without setting `CPACR_EL1.FPEN` (bits 20-21) to `0b11` in the boot stub, the first such call traps. With no exception vector table installed (`VBAR_EL1 == 0` at reset), the trap vectors to address `0x200` and the CPU spins forever on undefined instructions there — indistinguishable from a plain hang, with zero diagnostic output.

### Original project (C/NASM) still relevant

9. **"MZ" corrupts DL**: The MBR's first 2 bytes are `0x4D 0x5A`, which execute as `dec bp; pop dx`. The `pop dx` overwrites the boot-drive value in DL. Always set DL explicitly before INT 13h calls.
10. **512-byte MBR limit**: The MBR is strictly 512 bytes (including `0xAA55` at `0x1FE`). All real logic goes in `stage2.bin`.
11. **ARM64 UEFI SNP single-transmit limit**: The ARM64 virtio SNP driver only supports one `Transmit` per session because `Initialize` returns `EFI_UNSUPPORTED` (no buffer recycling). Send DHCPDISCOVER and accept the OFFER as final — do not attempt REQUEST/ACK. x86_64 e1000 supports the full handshake.
12. **ARM64 EDK2 error bit**: ARM64 firmware encodes EFI errors with bit 63 set (`0x8000000000000014` for `EFI_ALREADY_STARTED`). Always check both the plain constant and the error-bit variant when comparing status codes on ARM64.
13. **`virtio-net-pci` is the only ARM64 SNP NIC**: QEMU `qemu-system-aarch64` EDK2 firmware only detects SNP via `virtio-net-pci`. All other models (e1000, e1000e, rtl8139, etc.) return "No network adapters found".
14. **`EFI_OPEN_PROTOCOL_GET_PROTOCOL`**: Use `0x00000002` instead of `BY_HANDLE_PROTOCOL` (`0x00000001`) for SNP on ARM64 to avoid agent handle tracking issues.
15. **Non-null SNP.Receive pointers**: Some firmware (ARM64) requires `SrcAddr`, `DestAddr`, and `Protocol` to be non-null output pointers. Passing `null` causes `EFI_INVALID_PARAMETER`.
16. **`.comment` section causes OVMF Load Error**: Removed from stage2 with `objcopy -R .comment`. When producing PE/COFF from Rust UEFI targets, the linker may still add metadata sections; ensure they're stripped.
17. **QEMU virt ECAM moved to 64-bit address**: PCIe ECAM is at `0x4010_0000_0000` (QEMU virt v11+), not `0x3F00_0000`. Hardcoded in `arm64-bare/src/pci.rs`.
18. **PCI BARs unassigned without firmware**: On bare-metal, BARs read as 0. Must implement BAR sizing (write-all-1s) and resource allocation from PCI MMIO window (`0x1000_0000–0x3EFF_FFFF` on virt).
19. **AHCI requires HBA enable before port access**: Set GHC.AE (bit 31 of ABAR+0x04) before reading CAP, PI, SSTS.
20. **QEMU DTB at 0x4000_0000**: Load bare-metal binaries at `0x4020_0000` or higher to avoid overlap. Set in linker script / start.S.
21. **`llvm-objcopy` for ARM64 ELF→binary**: Host `objcopy` cannot handle ARM64 ELF. Use `llvm-objcopy -O binary`.
22. **`lld` required for ARM64 linking**: Host `ld` lacks `aarch64linux` emulation. Use `-fuse-ld=lld` with clang or `lld-link` for UEFI.

### Disk-image

23. **mtools required**: `mkfs.fat`, `mmd`, `mcopy` must be installed for FAT32 ESP creation. The Rust `disk-image` crate shells out to these tools.
24. **Partition table format**: 16-byte entry at offset `0x1BE`: `u8 status`, `[u8; 3] chs_start`, `u8 type`, `[u8; 3] chs_end`, `u32 lba_start`, `u32 sector_count`. All little-endian.
25. **Disk image size**: The MBR + stage2 occupy LBA 0–10. The FAT32 partition starts at LBA 11. PARTITION_LBA=11 in disk-image/src/main.rs. Update when stage2 size changes.

### BIOS stage2 (C, retained as-is)

26. **INT 13h register preservation**: `INT 13h` can clobber registers. Use C local variables instead of relying on register values across calls.
27. **DPB buffer size**: Must be set to `30` (`0x001E`) in the first word before AH=48h.
28. **Global data in `.data` for `--oformat=binary`**: Accessed globals must be in `.data` (not `.bss`) because `ld --oformat=binary` skips `.bss`. Both `dpb_buf` and `g_putc` use `__attribute__((section(".data")))`.
29. **`"di"` clobber for INT 13h AH=48h**: The call clobbers `%edi`. List `"di"` as a clobber in inline asm, otherwise GCC may keep the info pointer in `%edi` across the asm, corrupting memory.
30. **`__umoddi3` inline asm indexing**: With two outputs (`"=a", "=d"`) before three inputs, the divisor is `%4`, not `%3`. Using `%3` causes `divl %edx` (divide by EDX), which hangs when EDX=0.

31. **Stage2 10-sector limit (5120 bytes)**: The MBR loads 10 sectors (LBA 1-10, 5120 bytes) at physical 0x1000. Keep `stage2.bin` under this limit. Check with `stat -c%s bin/stage2.bin`.

32. **PXE/UNDI via INT 1Ah**: PXE entry is discovered via INT 1Ah AX=0x5650. Functions called via INT 1Ah AX=0, BX=function, ES:DI=parameter block. Function numbers: 0x0001=UNDI_STARTUP, 0x0003=UNDI_INITIALIZE, 0x0005=UNDI_OPEN, 0x000B=UNDI_GET_INFO, 0x0030=UDP_OPEN, 0x0034=UDP_TRANSMIT, 0x0036=UDP_RECEIVE.

33. **PXE inline asm clobbers**: For `int 0x1A` PXE calls, only AX and flags are preserved. Use `push`/`pop` for BX and ES inside the asm. List `"cc"` and `"memory"` as clobbers.

34. **PXE buffer in `.data`**: The 64-byte `pxe_buf` must be in `.data` (not `.bss`) because `ld --oformat=binary` skips `.bss`. Must be zeroed before each call via `pxe_clear()`.

35. **Frame buffers at fixed offset 0x1500**: DHCP and receive buffers at offset 0x1500 from DS=0x0100 (physical 0x2500) — past the max stage2 size of 0x1400 bytes. Hardcoded pointers avoid bloat from 1514+ byte buffers.

36. **UDP port fields in network byte order**: PXE UDP parameter structures use big-endian port numbers (`htons()`). `BufferLen` is in host byte order.

37. **e1000 is the BIOS test NIC**: Use `-nic user,model=e1000` for SeaBIOS PXE support in QEMU. virtio-net-pci also works. Other models may not have UNDI support.

38. **Single-transmit DHCP for BIOS**: Same pattern as ARM64 UEFI — send one DISCOVER via UDP_TRANSMIT, poll UDP_RECEIVE up to 100k iterations, accept OFFER as final. No REQUEST/ACK.

39. **Custom SeaBIOS for PXE testing**: The `run-bios-pxe` target builds a custom SeaBIOS (`make seabios`, cloned from GitHub) and uses it via `-bios`. This ensures the PXE/UNDI INT 1Ah handler stays resident after boot (the default SeaBIOS that ships with QEMU tears down the PXE stack after it boots from a non-PXE device). The build uses `git clone --depth=1` into `build/seabios` and runs `make defconfig` to produce a QEMU-optimized `bios.bin`. On real hardware workstations, the PXE stack remains resident in UMA regardless of boot source and no custom SeaBIOS is needed.

40. **QEMU e1000 I/O BAR is a stub**: QEMU's classic e1000 PCI I/O handler (`e1000_io_read`/`e1000_io_write`) is an empty stub — it returns 0 for all reads and ignores all writes. This means the direct e1000 driver in `pxe.c` (which uses the PCI I/O BAR for register access) can NOT work on QEMU. On real hardware, the I/O BAR software access protocol provides full register access. For QEMU testing, use `make run-uefi` (UEFI path works in QEMU) or `make run-bios-pxe` (BIOS PXE with custom SeaBIOS).
41. **UEFI keyboard input is non-blocking**: `EFI_SIMPLE_TEXT_INPUT_PROTOCOL.ReadKeyStroke` returns `EFI_NOT_READY` when no key is pressed. The menu polls it in a tight loop until a valid choice is received. Store the system table pointer globally so the polling function can reach `con_in` without threading it through every helper.
42. **BIOS menu reads from serial port**: The BIOS stage-2 uses serial output, so the menu reads from COM1 (0x3F8) rather than the keyboard. This matches the `-serial stdio` setup used by `run-bios` and `run-bios-pxe`. Real hardware with a serial console will work the same way; a VGA+keyboard setup would require a keyboard input path instead.

## Makefile Targets

```bash
make all                         # Build everything
make bios                        # Build MBR + stage2 (C/NASM)
make uefi                        # Build Rust UEFI binary (x86_64 + ARM64)
make arm64                       # Alias for uefi (ARM64 UEFI)
make bare-arm64                  # Build Rust ARM64 bare-metal
make combined                    # Build disk image (requires mtools)
make seabios                     # Build custom SeaBIOS (auto-cloned from GitHub)
make run-bios                    # BIOS mode via SeaBIOS (serial output; e1000 I/O not avail)
make run-bios-pxe                # BIOS PXE with custom iPXE ROM + custom SeaBIOS
make run-uefi                    # x86_64 UEFI from combined disk image
make run-uefi-arm64              # ARM64 UEFI from FAT directory
make run-bare-arm64              # ARM64 bare-metal + AHCI drive
make clean                       # Clean all artifacts (keeps SeaBIOS checkout)
make seabios-clean               # Remove SeaBIOS checkout
```

## Targets

| Target                          | Arch   | Firmware          | Binary                           |
| ------------------------------- | ------ | ----------------- | -------------------------------- |
| `x86_64-unknown-uefi`           | x86_64 | UEFI              | `bin/rustrapper.efi`               |
| `aarch64-unknown-uefi`          | ARM64  | UEFI              | `bin/rustrapper_arm64.efi`         |
| `aarch64-unknown-none`          | ARM64  | None (bare-metal) | `bin/rustrapper_arm64_bare.elf`    |
| `x86_64-linux-gnu` (disk-image) | Host   | —                 | `target/debug/disk-image`        |
| `i386-none-elf` (BIOS)          | x86-16 | BIOS              | `bin/bios.bin`, `bin/stage2.bin` |

## Tests

Run the full host-testable suite:

```bash
cargo test --workspace        # All 71 tests across all crates
cargo test --package common   # 22 tests (formatting, scan loop)
cargo test --package uefi     # 20 tests (type sizes, GUID values, constants, SNP mode layout, input protocol sizes)
cargo test --package arm64-bare  # 15 tests (pci_off, storage_name)
cargo test --package disk-image  # 14 tests (CHS geometry, MBR partition entries)
```

| Crate | Tests | What's tested |
|-------|-------|---------------|
| `common` | 22 | `format_hex` edge cases (zero, leading-zero suppression, all nibbles, 64-bit), `format_dec` (zero, round numbers, max u64, powers of 10), `DeviceInfo` construction, `scan_devices` with mock detectors (0/1/multiple devices, non-present skipping) |
| `uefi`/`efi` | 20 | Struct sizes under `repr(C)`, GUID byte values, `EFI_SUCCESS=0`, type consistency (UINTN=8 bytes, EFI_HANDLE=pointer size), GUID uniqueness, SNP mode layout, SNP transmit/receive function offsets, `EFI_SIMPLE_TEXT_INPUT_PROTOCOL` / `EFI_INPUT_KEY` sizes |
| `arm64-bare`/`pci` | 15 | `pci_off` bit-field encoding (bus/dev/func/offset combinations, max values), `storage_name` mapping (all 11 subclass codes, unknown fallback) |
| `disk-image` | 14 | `chs_from_lba` geometry (sector/head/cylinder boundaries), `build_mbr_partition` (bootable flag, type byte, LBA/sector count, CHS clamping at 1023/254/63), partition size invariants |

**Test architecture**: `common`/`disk-image` tests run directly on the host. `uefi` tests run on the host by guarding platform-specific code with `#[cfg(not(test))]` and using `#[cfg_attr(not(test), no_std)]` / `#[cfg_attr(not(test), no_main)]` so the standard test harness can drive them. `arm64-bare` tests similarly guard the ARM64 `global_asm!` entry point and the `extern "C" fn main` behind `#[cfg(not(test))]`, exposing only pure functions (`pci_off`, `storage_name`) for host testing.

The `print` module separates formatting from I/O: `format_hex`/`format_dec` write to caller-provided byte buffers and return `&str`, while `print_hex`/`print_dec` call `puts` through the global `PUTC_FN` callback. Tests exercise the pure formatting functions directly, avoiding the `static mut` callback.

## Tools Required

- **Rust**: `rustc`, `cargo` with targets `x86_64-unknown-uefi`, `aarch64-unknown-uefi`, `aarch64-unknown-none`
- **BIOS (C/NASM)**: `gcc`, `ld` (`-m elf_i386`), `nasm`, `objcopy`
- **Common**: `mkfs.fat`, `mmd`, `mcopy` (dosfstools + mtools)
- **Testing**: `qemu-system-x86_64` (with OVMF), `qemu-system-aarch64` (with `/usr/share/edk2/aarch64/QEMU_EFI.fd`)
