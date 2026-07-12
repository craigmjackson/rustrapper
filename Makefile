# Rustrapper - Rust Bootloader
# Targets: all, i386-bios, x86_64-uefi, aarch64-uefi, aarch64-bare
# Uses cargo for Rust crates, nasm+gcc for legacy BIOS parts

CARGO    := cargo
NASM     := nasm
CC       := gcc
CFLAGS   := -ffreestanding -fno-stack-protector -mno-sse -mno-mmx \
            -fno-pic -fno-strict-aliasing -nostartfiles -Wall -Wextra -std=c11 -Os -Ibios
BIN      := bin
BIOS_SRC := bios

# x86_64 UEFI target
UEFI_TARGET   := x86_64-unknown-uefi
# ARM64 UEFI target (requires `rustup target add aarch64-unknown-uefi`)
UEFI_ARM64_TARGET := aarch64-unknown-uefi
# ARM64 bare-metal target
BARE_ARM64_TARGET := aarch64-unknown-none

.PHONY: all i386-bios x86_64-uefi aarch64-uefi aarch64-bare x86_64-seabios \
        run-i386-bios run-x86_64-uefi run-aarch64-uefi run-aarch64-bare clean \
        x86_64-uefi-rom i386-bios-rom i386-bios-rust-rom \
        run-x86_64-uefi-rom run-i386-bios-rom run-i386-bios-rust-rom run-i386-bios-rom-pxe \
        run-i386-bios-ipxe x86_64-seabios-clean i386-bios-rust run-i386-bios-rust

all: i386-bios x86_64-uefi aarch64-uefi aarch64-bare

# Create output directory
$(BIN):
	mkdir -p $(BIN)

# ── BIOS Rust payload (32-bit protected mode, loaded at 1 MB) ────
# Requires nightly for -Zjson-target-spec and -Zbuild-std=core
BIOS_RUST_TARGET := $(CURDIR)/targets/i386-unknown-none.json
CARGO_NIGHTLY    := cargo +nightly

$(BIN)/rust_payload.bin: $(shell find bios-rust common -name '*.rs') \
                         bios-rust/link.ld Cargo.toml $(BIOS_RUST_TARGET) | $(BIN)
	CARGO_TARGET_DIR=target RUSTFLAGS="-C link-arg=-T$(CURDIR)/bios-rust/link.ld -C link-arg=-N" \
		$(CARGO_NIGHTLY) build -Zjson-target-spec -Zbuild-std=core \
		--target $(BIOS_RUST_TARGET) --package bios-rust --release
	objcopy -O binary target/i386-unknown-none/release/bios-rust $@

# Combined stage2 = entry stub + Rust payload (assembled by NASM, which
# incbins the payload so it can compute copy offsets at assembly time)
$(BIN)/stage2_entry.bin: $(BIN)/rust_payload.bin | $(BIN)
	cd $(BIOS_SRC) && $(NASM) -f bin -o ../$@ stage2_entry.nasm

# Experimental: build the Rust-based BIOS stage2 instead of the C one
i386-bios-rust: $(BIN)/bios.bin $(BIN)/stage2_entry.bin

# ── BIOS MBR (stage-1, 512 bytes, NASM) ──────────────────────────
# NOTE: when switching to the Rust payload, update the sector count
# in mbr.asm (line 69: `dw 14`) to cover the full stage2_entry.bin.
$(BIN)/bios.bin: $(BIOS_SRC)/mbr.asm | $(BIN)
	$(NASM) -f bin -o $@ $<

# ── BIOS Stage-2 (C with -m16, flat binary) ──────────────────────
S2_SRC    := $(BIOS_SRC)/stage2.c $(BIOS_SRC)/div64.c
S2_COMMON := $(BIOS_SRC)/print.c $(BIOS_SRC)/scan.c $(BIOS_SRC)/pxe.c
S2_BUILD  := build/stage2

$(S2_BUILD):
	mkdir -p $(S2_BUILD)

$(S2_BUILD)/stage2.o: $(S2_SRC) | $(S2_BUILD)
	$(CC) -m16 $(CFLAGS) -c $(BIOS_SRC)/stage2.c -o $(S2_BUILD)/stage2.o

$(S2_BUILD)/div64.o: $(BIOS_SRC)/div64.c | $(S2_BUILD)
	$(CC) -m16 $(CFLAGS) -c $(BIOS_SRC)/div64.c -o $(S2_BUILD)/div64.o

$(S2_BUILD)/print_bios.o: $(BIOS_SRC)/print.c | $(S2_BUILD)
	$(CC) -m16 $(CFLAGS) -c $(BIOS_SRC)/print.c -o $(S2_BUILD)/print_bios.o

$(S2_BUILD)/scan_bios.o: $(BIOS_SRC)/scan.c | $(S2_BUILD)
	$(CC) -m16 $(CFLAGS) -c $(BIOS_SRC)/scan.c -o $(S2_BUILD)/scan_bios.o

$(S2_BUILD)/pxe_bios.o: $(BIOS_SRC)/pxe.c | $(S2_BUILD)
	$(CC) -m16 $(CFLAGS) -c $(BIOS_SRC)/pxe.c -o $(S2_BUILD)/pxe_bios.o

$(BIN)/stage2.bin: $(S2_BUILD)/stage2.o $(S2_BUILD)/div64.o \
                   $(S2_BUILD)/print_bios.o $(S2_BUILD)/scan_bios.o \
                   $(S2_BUILD)/pxe_bios.o | $(BIN)
	ld -m elf_i386 -e _start -Ttext=0x0 -N --oformat=binary \
		$(S2_BUILD)/stage2.o $(S2_BUILD)/print_bios.o \
		$(S2_BUILD)/scan_bios.o $(S2_BUILD)/pxe_bios.o \
		$(S2_BUILD)/div64.o -o $@

i386-bios: $(BIN)/bios.bin $(BIN)/stage2.bin

# ── x86_64 UEFI ──────────────────────────────────────────────────
# The common crate does not need a separate -I path; Cargo handles deps
$(BIN)/rustrapper.efi: $(shell find uefi common -name '*.rs') Cargo.toml | $(BIN)
	CARGO_TARGET_DIR=target RUSTFLAGS="-C link-args=/NODEFAULTLIB" \
		$(CARGO) build --target $(UEFI_TARGET) --package uefi --release
	cp target/$(UEFI_TARGET)/release/uefi.efi $@

# ── ARM64 UEFI ───────────────────────────────────────────────────
$(BIN)/rustrapper_arm64.efi: $(shell find uefi common -name '*.rs') Cargo.toml | $(BIN)
	CARGO_TARGET_DIR=target \
		$(CARGO) build --target $(UEFI_ARM64_TARGET) --package uefi --release
	cp target/$(UEFI_ARM64_TARGET)/release/uefi.efi $@

x86_64-uefi: $(BIN)/rustrapper.efi
aarch64-uefi: $(BIN)/rustrapper_arm64.efi

# ── UEFI PCI Expansion ROM ──────────────────────────────────────
ROMWRAP_BIN := target/debug/romwrap

$(ROMWRAP_BIN): $(shell find romwrap -name '*.rs') Cargo.toml
	$(CARGO) build --package romwrap

$(BIN)/rustrapper_efi.rom: $(BIN)/rustrapper.efi $(ROMWRAP_BIN) | $(BIN)
	$(CARGO) run --package romwrap -- $(BIN)/rustrapper.efi $@

x86_64-uefi-rom: $(BIN)/rustrapper_efi.rom

# ── BIOS PCI Expansion ROM ────────────────────────────────
$(BIN)/rustrapper_bios.rom: $(BIN)/stage2.bin $(ROMWRAP_BIN) | $(BIN)
	$(CARGO) run --package romwrap -- $(BIN)/stage2.bin $@ --bios

i386-bios-rom: $(BIN)/rustrapper_bios.rom

# BIOS option ROM from Rust payload: the raw 32-bit PM binary wrapped
# as a legacy BIOS option ROM. Like the C BIOS ROM, the init routine
# at 0x34 is a no-op (xor ax,ax; retf); the payload is just data.
# The Rust payload itself runs from disk via the stage2 entry stub.
$(BIN)/rustrapper_bios_rust.rom: $(BIN)/rust_payload.bin $(ROMWRAP_BIN) | $(BIN)
	$(CARGO) run --package romwrap -- $(BIN)/rust_payload.bin $@ --bios

i386-bios-rust-rom: $(BIN)/rustrapper_bios_rust.rom

# ── ARM64 Bare-metal ──────────────────────────────────────────────
$(BIN)/rustrapper_arm64_bare.elf: $(shell find arm64-bare common -name '*.rs') \
                                arm64-bare/link.ld Cargo.toml | $(BIN)
	CARGO_TARGET_DIR=target RUSTFLAGS="-C link-arg=-T$(CURDIR)/arm64-bare/link.ld -C link-arg=-N" \
		$(CARGO) build --target $(BARE_ARM64_TARGET) --package arm64-bare --release
	cp target/$(BARE_ARM64_TARGET)/release/arm64-bare $@

aarch64-bare: $(BIN)/rustrapper_arm64_bare.elf

# ── BIOS disk image (MBR + stage2, no FAT32) ─────────────────────
$(BIN)/bios.img: $(BIN)/bios.bin $(BIN)/stage2.bin | $(BIN)
	dd if=/dev/zero bs=1M count=64 of=$@ 2>/dev/null
	dd if=$(BIN)/bios.bin of=$@ conv=notrunc 2>/dev/null
	dd if=$(BIN)/stage2.bin of=$@ bs=512 seek=1 conv=notrunc 2>/dev/null

# BIOS disk image with Rust stage2 payload
$(BIN)/bios-rust.img: $(BIN)/bios.bin $(BIN)/stage2_entry.bin | $(BIN)
	dd if=/dev/zero bs=1M count=64 of=$@ 2>/dev/null
	dd if=$(BIN)/bios.bin of=$@ conv=notrunc 2>/dev/null
	dd if=$(BIN)/stage2_entry.bin of=$@ bs=512 seek=1 conv=notrunc 2>/dev/null

# ── SeaBIOS (custom build with PXE stack kept) ──────────────────
SEABIOS_DIR := build/seabios
SEABIOS_BIN := $(SEABIOS_DIR)/out/bios.bin

$(SEABIOS_DIR):
	git clone --depth=1 https://github.com/coreboot/seabios.git $(SEABIOS_DIR)

$(SEABIOS_DIR)/.config: | $(SEABIOS_DIR)
	$(MAKE) -C $(SEABIOS_DIR) defconfig

$(SEABIOS_BIN): $(SEABIOS_DIR)/.config
	$(MAKE) -C $(SEABIOS_DIR)

x86_64-seabios: $(SEABIOS_BIN)

# ── Run targets ──────────────────────────────────────────────────
# BIOS e1000 direct driver doesn't work on QEMU due to stub I/O BAR (see AGENTS.md).
# Use run-x86_64-uefi for QEMU testing, or real hardware for BIOS e1000.
run-i386-bios: $(BIN)/bios.img
	@echo "Note: BIOS e1000 driver requires real hardware (QEMU I/O BAR is a stub)."
	@echo "      Use run-x86_64-uefi for QEMU testing, or run-i386-bios-ipxe with custom SeaBIOS."
	qemu-system-x86_64 -drive file=$(BIN)/bios.img,format=raw -nic user,model=e1000 -nographic

run-i386-bios-rust: $(BIN)/bios-rust.img
	qemu-system-x86_64 -drive file=$(BIN)/bios-rust.img,format=raw -nic user,model=e1000 -nographic

# BIOS PXE test with custom iPXE ROM (requires custom SeaBIOS built by the
# 'x86_64-seabios' target to keep the PXE/UNDI INT 1Ah handler resident).
run-i386-bios-ipxe: $(BIN)/bios.img $(SEABIOS_BIN)
	qemu-system-x86_64 -bios $(SEABIOS_BIN) -drive file=$(BIN)/bios.img,format=raw \
		-netdev user,id=net0 \
		-device e1000,romfile=/tmp/ipxe/src/bin/8086100e.rom,netdev=net0 \
		-serial stdio -display none -m 64 -no-reboot

run-x86_64-uefi: $(BIN)/rustrapper.efi
	mkdir -p EFI/BOOT
	cp $(BIN)/rustrapper.efi EFI/BOOT/BOOTX64.EFI
	qemu-system-x86_64 -bios /usr/share/edk2/x64/OVMF.4m.fd \
		-drive file=fat:rw:.,format=raw -nic user,model=e1000 -nographic

run-x86_64-uefi-rom: $(BIN)/rustrapper.efi $(BIN)/rustrapper_efi.rom
	mkdir -p EFI/BOOT
	cp $(BIN)/rustrapper.efi EFI/BOOT/BOOTX64.EFI
	qemu-system-x86_64 -bios /usr/share/edk2/x64/OVMF.4m.fd \
		-drive file=fat:rw:.,format=raw \
		-netdev user,id=net0 \
		-device e1000,romfile=$(BIN)/rustrapper_efi.rom,netdev=net0 \
		-nographic

# BIOS option ROM: stage2.bin wrapped as PCI option ROM with PCIR header.
# The ROM is detected by SeaBIOS which calls the init routine (minimal:
# just returns). The stage2 code uses the INT 1Ah PXE/UNDI API if a PXE
# stack is available.  Direct e1000 I/O BAR is a QEMU stub (see AGENTS.md
# gotcha #40), so QEMU testing requires a separate PXE stack.
#
# Two run modes (QEMU only, not real hardware):
#   make run-i386-bios-rom         — uses stock SeaBIOS, no PXE (no networking)
#   make run-i386-bios-rom-pxe     — uses custom SeaBIOS + iPXE ROM on 2nd NIC
PXE_ROM ?= /tmp/ipxe/src/bin/8086100e.rom
run-i386-bios-rom: $(BIN)/bios.img $(BIN)/rustrapper_bios.rom
	qemu-system-x86_64 -drive file=$(BIN)/bios.img,format=raw \
		-netdev user,id=net0 \
		-device e1000,romfile=$(BIN)/rustrapper_bios.rom,netdev=net0 \
		-nographic

run-i386-bios-rom-pxe: $(BIN)/bios.img $(BIN)/rustrapper_bios.rom $(SEABIOS_BIN)
	@test -f $(PXE_ROM) || { echo "Error: $(PXE_ROM) not found. Build iPXE first (see Makefile comments."; exit 1; }
	qemu-system-x86_64 -bios $(SEABIOS_BIN) -drive file=$(BIN)/bios.img,format=raw \
		-netdev user,id=net0 \
		-device e1000,romfile=$(BIN)/rustrapper_bios.rom,netdev=net0 \
		-netdev user,id=net1 \
		-device e1000,romfile=$(PXE_ROM),netdev=net1 \
		-serial stdio -display none -m 64 -no-reboot

# BIOS option ROM from Rust payload: rust_payload.bin wrapped as PCI option ROM.
# Like run-i386-bios-rom, the init routine is a no-op; the ROM just carries
# the Rust payload as data. The Rust code actually runs from disk via the
# stage2 entry stub loaded by the MBR.
run-i386-bios-rust-rom: $(BIN)/bios-rust.img $(BIN)/rustrapper_bios_rust.rom
	qemu-system-x86_64 -drive file=$(BIN)/bios-rust.img,format=raw \
		-netdev user,id=net0 \
		-device e1000,romfile=$(BIN)/rustrapper_bios_rust.rom,netdev=net0 \
		-nographic

run-aarch64-uefi: $(BIN)/rustrapper_arm64.efi
	mkdir -p EFI/BOOT
	cp $< EFI/BOOT/BOOTAA64.EFI
	qemu-system-aarch64 -machine virt -cpu max \
		-bios /usr/share/edk2/aarch64/QEMU_EFI.fd \
		-drive file=fat:rw:.,format=raw -nic user,model=virtio-net-pci -nographic

test.img:
	qemu-img create -f raw $@ 64M 2>/dev/null || dd if=/dev/zero bs=1M count=64 of=$@ 2>/dev/null

run-aarch64-bare: $(BIN)/rustrapper_arm64_bare.elf test.img
	qemu-system-aarch64 -M virt -cpu max -kernel $< \
		-drive file=test.img,format=raw,if=none,id=drive0 \
		-device ahci,id=ahci \
		-device ide-hd,bus=ahci.0,drive=drive0 \
		-nic user,model=e1000 -nographic

# ── Clean ────────────────────────────────────────────────────────
x86_64-seabios-clean:
	rm -rf $(SEABIOS_DIR)

clean: x86_64-seabios-clean
	rm -rf $(BIN)/* build/
	rm -rf target/ EFI/
