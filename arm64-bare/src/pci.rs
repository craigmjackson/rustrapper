use core::ptr::{read_volatile, write_volatile};

use common::print::{print_dec, print_hex, putc, puts};

const ECAM_BASE: u64 = 0x4010_0000_00;

pub const fn pci_off(bus: u8, dev: u8, func: u8, offset: u8) -> u64 {
    (bus as u64) << 20 | (dev as u64) << 15 | (func as u64) << 12 | (offset as u64)
}

fn pci_read32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let addr = (ECAM_BASE + pci_off(bus, dev, func, offset)) as *const u32;
    unsafe { read_volatile(addr) }
}

fn pci_write32(bus: u8, dev: u8, func: u8, offset: u8, val: u32) {
    let addr = (ECAM_BASE + pci_off(bus, dev, func, offset)) as *mut u32;
    unsafe { write_volatile(addr, val) };
}

fn pci_read16(bus: u8, dev: u8, func: u8, offset: u8) -> u16 {
    let addr = (ECAM_BASE + pci_off(bus, dev, func, offset)) as *const u16;
    unsafe { read_volatile(addr) }
}

fn pci_write16(bus: u8, dev: u8, func: u8, offset: u8, val: u16) {
    let addr = (ECAM_BASE + pci_off(bus, dev, func, offset)) as *mut u16;
    unsafe { write_volatile(addr, val) };
}

fn pci_vid(bus: u8, dev: u8, func: u8) -> u16 {
    pci_read32(bus, dev, func, 0) as u16
}
fn pci_did(bus: u8, dev: u8, func: u8) -> u16 {
    (pci_read32(bus, dev, func, 0) >> 16) as u16
}
fn pci_class(bus: u8, dev: u8, func: u8) -> u8 {
    (pci_read32(bus, dev, func, 8) >> 24) as u8
}
fn pci_subclass(bus: u8, dev: u8, func: u8) -> u8 {
    ((pci_read32(bus, dev, func, 8) >> 16) & 0xFF) as u8
}

fn pci_read8(bus: u8, dev: u8, func: u8, offset: u8) -> u8 {
    let addr = (ECAM_BASE + pci_off(bus, dev, func, offset)) as *const u8;
    unsafe { read_volatile(addr) }
}

static mut MMIO_NEXT: u64 = 0x3E00_0000;

fn pci_enable_bars(bus: u8, dev: u8, func: u8) {
    let cmd = pci_read16(bus, dev, func, 0x04);
    if cmd & 0x03 != 0 {
        return;
    }

    puts("    Enabling PCI device...\n");
    let mut b = 0;
    while b < 6 {
        let orig = pci_read32(bus, dev, func, 0x10 + b * 4);
        if orig == 0xFFFF_FFFF {
            b += 1;
            continue;
        }
        let is_64 = (orig & 0x06) == 0x04;
        let orig_hi = if is_64 {
            pci_read32(bus, dev, func, 0x10 + b * 4 + 4)
        } else {
            0
        };

        pci_write32(bus, dev, func, 0x10 + b * 4, 0xFFFF_FFFF);
        if is_64 {
            pci_write32(bus, dev, func, 0x10 + b * 4 + 4, 0xFFFF_FFFF);
        }
        let sz = pci_read32(bus, dev, func, 0x10 + b * 4);
        pci_write32(bus, dev, func, 0x10 + b * 4, orig);
        if is_64 {
            pci_write32(bus, dev, func, 0x10 + b * 4 + 4, orig_hi);
        }

        let mask = sz & !0xF;
        if mask == 0 {
            b += 1;
            continue;
        }
        let size = (!mask).wrapping_add(1);
        let addr = unsafe {
            let next = MMIO_NEXT;
            let aligned = (next + size as u64 - 1) & !(size as u64 - 1);
            MMIO_NEXT = aligned + size as u64;
            aligned
        };

        pci_write32(
            bus, dev, func, 0x10 + b * 4,
            (addr as u32 & !0xF) | (orig & 0xF),
        );
        if is_64 {
            pci_write32(bus, dev, func, 0x10 + b * 4 + 4, (addr >> 32) as u32);
            b += 1;
        }

        puts("      BAR");
        print_dec(b as u64);
        puts(": 0x");
        print_hex(addr, 16);
        if is_64 {
            puts(" (64-bit)");
        }
        puts(" size=0x");
        print_hex(size as u64, 8);
        putc(b'\n');

        b += 1;
    }

    pci_write16(bus, dev, func, 0x04, pci_read16(bus, dev, func, 0x04) | 0x07);
    puts("    Device enabled.\n");
}

pub fn storage_name(subclass: u8) -> &'static str {
    match subclass {
        0x00 => "SCSI",
        0x01 => "IDE",
        0x02 => "Floppy",
        0x03 => "IPI",
        0x04 => "RAID",
        0x05 => "ATA",
        0x06 => "SATA (AHCI)",
        0x07 => "SAS",
        0x08 => "NVMe",
        0x09 => "USB",
        0x0A => "SD",
        _ => "Other",
    }
}

fn probe_ahci(bus: u8, dev: u8, func: u8) {
    let bar5 = pci_read32(bus, dev, func, 0x24);
    let is_64 = (bar5 & 0x06) == 0x04;
    let abar: u64 = if is_64 {
        (bar5 & !0xF) as u64 | ((pci_read32(bus, dev, func, 0x28) as u64) << 32)
    } else {
        (bar5 & !0xF) as u64
    };

    if abar == 0 {
        puts("      ABAR = 0 after enable!\n");
        return;
    }

    puts("      ABAR: 0x");
    print_hex(abar, 16);
    putc(b'\n');

    let ghc = (abar + 0x04) as *mut u32;
    unsafe {
        write_volatile(ghc, read_volatile(ghc) | 0x8000_0000);
        for _ in 0..100 {
            core::hint::spin_loop();
        }
    }

    let cap = unsafe { read_volatile(abar as *const u32) };
    let np = (cap & 0x1F) + 1;
    let pi = unsafe { read_volatile((abar + 0x0C) as *const u32) };

    puts("      Ports: ");
    print_dec(np as u64);
    putc(b'\n');

    for p in 0..np.min(32) {
        if pi & (1u32 << p) == 0 {
            continue;
        }
        let port_base = abar + 0x100 + (p as u64) * 0x80;
        let ssts = unsafe { read_volatile((port_base + 0x28) as *const u32) };
        let det = ssts & 0x0F;

        puts("        Port ");
        print_dec(p as u64);
        puts(": ");
        if det == 3 {
            puts("device present (no AHCI data structures)\n");
        } else {
            puts("empty\n");
        }
    }
}

fn describe_pci_device(cls: u8, sub: u8, dev: u8) {
    if cls == 0x01 {
        puts(" (Mass storage: ");
        puts(storage_name(sub));
        putc(b')');
    } else if cls == 0x02 {
        puts(" (Network)");
    } else if cls == 0x03 {
        puts(" (Display)");
    } else if cls == 0x06 && sub == 0x00 {
        let rev = (pci_read32(0, dev, 0, 0x08) & 0xFF) as u8;
        puts(" (PCIe host bridge, rev=0x");
        print_hex(rev as u64, 2);
        putc(b')');
    } else {
        puts(" (class=0x");
        print_hex(cls as u64, 2);
        puts(" sub=0x");
        print_hex(sub as u64, 2);
        putc(b')');
    }
}

pub fn pci_print_all() {
    puts("PCI device scan:\n");
    for dev in 0..32u8 {
        let id = pci_read32(0, dev, 0, 0);
        if id == 0xFFFF_FFFF {
            continue;
        }

        let cls = pci_class(0, dev, 0);
        let sub = pci_subclass(0, dev, 0);
        let hdr = pci_read8(0, dev, 0, 0x0E);

        puts("  PCI device ");
        print_dec(dev as u64);
        puts(": vendor=0x");
        print_hex(pci_vid(0, dev, 0) as u64, 4);
        puts(" device=0x");
        print_hex(pci_did(0, dev, 0) as u64, 4);
        describe_pci_device(cls, sub, dev);
        putc(b'\n');

        if hdr & 0x80 != 0 {
            for fn_ in 1..8u8 {
                let fid = pci_read32(0, dev, fn_, 0);
                if fid == 0xFFFF_FFFF {
                    continue;
                }
                let fcls = pci_class(0, dev, fn_);
                let fsub = pci_subclass(0, dev, fn_);
                puts("    Func ");
                print_dec(fn_ as u64);
                puts(": vendor=0x");
                print_hex(pci_vid(0, dev, fn_) as u64, 4);
                puts(" device=0x");
                print_hex(pci_did(0, dev, fn_) as u64, 4);

                if fcls == 0x01 {
                    puts(" (Mass storage: ");
                    puts(storage_name(fsub));
                    putc(b')');
                } else {
                    puts(" (other)");
                }
                putc(b'\n');
            }
        }
    }
}

pub fn detect_device(index: usize, info: &mut common::scan::DeviceInfo) -> bool {
    let mut count = 0usize;
    for dev in 0..32u8 {
        let id = pci_read32(0, dev, 0, 0);
        if id == 0xFFFF_FFFF {
            continue;
        }

        let cls = pci_class(0, dev, 0);
        let sub = pci_subclass(0, dev, 0);
        let hdr = pci_read8(0, dev, 0, 0x0E);

        if cls == 0x01 {
            if count == index {
                info.index = dev * 8;
                info.present = true;
                info.removable = false;
                info.description = Some(storage_name(sub));
                info.block_size = 512;
                info.block_count = 0;
                pci_enable_bars(0, dev, 0);
                if sub == 0x06 {
                    probe_ahci(0, dev, 0);
                }
                return true;
            }
            count += 1;
        }

        if hdr & 0x80 != 0 {
            for fn_ in 1..8u8 {
                let fid = pci_read32(0, dev, fn_, 0);
                if fid == 0xFFFF_FFFF {
                    continue;
                }
                let fcls = pci_class(0, dev, fn_);
                let fsub = pci_subclass(0, dev, fn_);

                if fcls == 0x01 {
                    if count == index {
                        info.index = dev * 8 + fn_;
                        info.present = true;
                        info.removable = false;
                        info.description = Some(storage_name(fsub));
                        info.block_size = 512;
                        info.block_count = 0;
                        pci_enable_bars(0, dev, fn_);
                        if fsub == 0x06 {
                            probe_ahci(0, dev, fn_);
                        }
                        return true;
                    }
                    count += 1;
                }
            }
        }
    }
    false
}
