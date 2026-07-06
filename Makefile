# Strapper - Rust Bootloader
# Targets: all, bios, uefi, arm64, bare-arm64, combined
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

.PHONY: all bios uefi arm64 bare-arm64 combined \
        run-bios run-uefi run-uefi-arm64 run-bare-arm64 clean

all: combined arm64 bare-arm64

# Create output directory
$(BIN):
	mkdir -p $(BIN)

# ── BIOS MBR (stage-1, 512 bytes, NASM) ──────────────────────────
$(BIN)/bios.bin: $(BIOS_SRC)/mbr.asm | $(BIN)
	$(NASM) -f bin -o $@ $<

# ── BIOS Stage-2 (C with -m16, flat binary) ──────────────────────
S2_SRC    := $(BIOS_SRC)/stage2.c $(BIOS_SRC)/div64.c
S2_COMMON := $(BIOS_SRC)/print.c $(BIOS_SRC)/scan.c
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

$(BIN)/stage2.bin: $(S2_BUILD)/stage2.o $(S2_BUILD)/div64.o \
                   $(S2_BUILD)/print_bios.o $(S2_BUILD)/scan_bios.o | $(BIN)
	ld -m elf_i386 -e _start -Ttext=0x0 -N --oformat=binary \
		$(S2_BUILD)/stage2.o $(S2_BUILD)/print_bios.o \
		$(S2_BUILD)/scan_bios.o $(S2_BUILD)/div64.o -o $@

bios: $(BIN)/bios.bin $(BIN)/stage2.bin

# ── x86_64 UEFI ──────────────────────────────────────────────────
# The common crate does not need a separate -I path; Cargo handles deps
$(BIN)/strapper.efi: $(shell find uefi common -name '*.rs') Cargo.toml | $(BIN)
	CARGO_TARGET_DIR=target RUSTFLAGS="-C link-args=/NODEFAULTLIB" \
		$(CARGO) build --target $(UEFI_TARGET) --package uefi --release
	cp target/$(UEFI_TARGET)/release/uefi.efi $@

# ── ARM64 UEFI ───────────────────────────────────────────────────
$(BIN)/strapper_arm64.efi: $(shell find uefi common -name '*.rs') Cargo.toml | $(BIN)
	CARGO_TARGET_DIR=target \
		$(CARGO) build --target $(UEFI_ARM64_TARGET) --package uefi --release
	cp target/$(UEFI_ARM64_TARGET)/release/uefi.efi $@

uefi: $(BIN)/strapper.efi
arm64: $(BIN)/strapper_arm64.efi

# ── ARM64 Bare-metal ──────────────────────────────────────────────
$(BIN)/strapper_arm64_bare.elf: $(shell find arm64-bare common -name '*.rs') \
                                arm64-bare/link.ld Cargo.toml | $(BIN)
	CARGO_TARGET_DIR=target RUSTFLAGS="-C link-arg=-T$(CURDIR)/arm64-bare/link.ld -C link-arg=-N" \
		$(CARGO) build --target $(BARE_ARM64_TARGET) --package arm64-bare --release
	cp target/$(BARE_ARM64_TARGET)/release/arm64-bare $@

bare-arm64: $(BIN)/strapper_arm64_bare.elf

# ── Combined disk image ──────────────────────────────────────────
DISK_IMAGE_BIN := target/debug/disk-image

$(DISK_IMAGE_BIN): $(shell find disk-image -name '*.rs') Cargo.toml
	$(CARGO) build --package disk-image

$(BIN)/bootloader.combined: $(BIN)/bios.bin $(BIN)/stage2.bin \
                            $(BIN)/strapper.efi $(DISK_IMAGE_BIN) | $(BIN)
	$(CARGO) run --package disk-image -- \
		$(BIN)/bios.bin $(BIN)/stage2.bin $(BIN)/strapper.efi $@

combined: $(BIN)/bootloader.combined

# ── Run targets ──────────────────────────────────────────────────
run-bios: $(BIN)/bootloader.combined
	qemu-system-x86_64 -drive file=$<,format=raw -net none -nographic

run-uefi: $(BIN)/bootloader.combined
	qemu-system-x86_64 -bios /usr/share/edk2/x64/OVMF.4m.fd \
		-drive file=$<,format=raw -net none -nographic

run-uefi-arm64: $(BIN)/strapper_arm64.efi
	mkdir -p EFI/BOOT
	cp $< EFI/BOOT/BOOTAA64.EFI
	qemu-system-aarch64 -machine virt -cpu max \
		-bios /usr/share/edk2/aarch64/QEMU_EFI.fd \
		-drive file=fat:rw:.,format=raw -net none -nographic

test.img:
	qemu-img create -f raw $@ 64M 2>/dev/null || dd if=/dev/zero bs=1M count=64 of=$@ 2>/dev/null

run-bare-arm64: $(BIN)/strapper_arm64_bare.elf test.img
	qemu-system-aarch64 -M virt -cpu max -kernel $< \
		-drive file=test.img,format=raw,if=none,id=drive0 \
		-device ahci,id=ahci \
		-device ide-hd,bus=ahci.0,drive=drive0 \
		-net none -nographic

# ── Clean ────────────────────────────────────────────────────────
clean:
	rm -rf $(BIN)/* build/
	rm -rf target/ EFI/
