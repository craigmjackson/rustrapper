#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(dead_code))]

mod uart;
mod pci;
#[cfg(not(test))]
mod net;
#[cfg(not(test))]
mod mem;
#[cfg(not(test))]
mod loader;

#[cfg(not(test))]
use core::panic::PanicInfo;

#[cfg(not(test))]
use common::menu::{show_menu, MenuAction};
#[cfg(not(test))]
use common::print;
#[cfg(not(test))]
use common::scan;

#[cfg(not(test))]
core::arch::global_asm!(
    ".section .text.boot",
    ".globl _start",
    "_start:",
    "  ldr x30, =_stack_top",
    "  mov sp, x30",
    "  mrs x9, cpacr_el1",
    "  orr x9, x9, #(3 << 20)", // FPEN = 0b11: no trapping of FP/SIMD at EL0/EL1
    "  msr cpacr_el1, x9",
    "  isb",
    "  ldr x0, =_bss_start",
    "  ldr x1, =_bss_end",
    "  cmp x0, x1",
    "  b.hs 2f",
    "1:  str xzr, [x0], #8",
    "    cmp x0, x1",
    "    b.lo 1b",
    "2:  bl main",
    "    wfi",
    "    b 2b",
);

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn main() -> ! {
    print::init(uart::putc);
    print::puts("\nRustrapper ARM64 Bare-Metal\n");
    pci::pci_print_all();
    loop {
        match show_menu(common::print::puts, common::print::putc, uart::getc) {
            MenuAction::StorageScan => {
                print::puts("\nStorage devices:\n");
                scan::scan_devices(pci::detect_device);
            }
            MenuAction::NetworkBoot => {
                print::puts("\n");
                net::scan_network();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pci;

    #[test]
    fn test_pci_off_zero() {
        let off = pci::pci_off(0, 0, 0, 0);
        assert_eq!(off, 0);
    }

    #[test]
    fn test_pci_off_bus() {
        let off = pci::pci_off(1, 0, 0, 0);
        assert_eq!(off, 1 << 20);
    }

    #[test]
    fn test_pci_off_device() {
        let off = pci::pci_off(0, 5, 0, 0);
        assert_eq!(off, 5 << 15);
    }

    #[test]
    fn test_pci_off_function() {
        let off = pci::pci_off(0, 0, 3, 0);
        assert_eq!(off, 3 << 12);
    }

    #[test]
    fn test_pci_off_register() {
        let off = pci::pci_off(0, 0, 0, 0x10);
        assert_eq!(off, 0x10);
    }

    #[test]
    fn test_pci_off_combined() {
        let off = pci::pci_off(2, 7, 1, 0x24);
        assert_eq!(off, (2 << 20) | (7 << 15) | (1 << 12) | 0x24);
    }

    #[test]
    fn test_pci_off_max() {
        let off = pci::pci_off(255, 31, 7, 0xFF);
        let expected: u64 = (255u64 << 20) | (31 << 15) | (7 << 12) | 0xFF;
        assert_eq!(off, expected);
    }

    #[test]
    fn test_storage_name_scsi() {
        assert_eq!(pci::storage_name(0x00), "SCSI");
    }

    #[test]
    fn test_storage_name_ide() {
        assert_eq!(pci::storage_name(0x01), "IDE");
    }

    #[test]
    fn test_storage_name_ahci() {
        assert_eq!(pci::storage_name(0x06), "SATA (AHCI)");
    }

    #[test]
    fn test_storage_name_nvme() {
        assert_eq!(pci::storage_name(0x08), "NVMe");
    }

    #[test]
    fn test_storage_name_usb() {
        assert_eq!(pci::storage_name(0x09), "USB");
    }

    #[test]
    fn test_storage_name_sd() {
        assert_eq!(pci::storage_name(0x0A), "SD");
    }

    #[test]
    fn test_storage_name_unknown() {
        assert_eq!(pci::storage_name(0xFF), "Other");
    }

    #[test]
    fn test_storage_name_default() {
        assert_eq!(pci::storage_name(0x0B), "Other");
    }

    #[test]
    fn test_storage_name_floppy() {
        assert_eq!(pci::storage_name(0x02), "Floppy");
    }

    #[test]
    fn test_storage_name_ipi() {
        assert_eq!(pci::storage_name(0x03), "IPI");
    }

    #[test]
    fn test_storage_name_raid() {
        assert_eq!(pci::storage_name(0x04), "RAID");
    }

    #[test]
    fn test_storage_name_ata() {
        assert_eq!(pci::storage_name(0x05), "ATA");
    }

    #[test]
    fn test_storage_name_sas() {
        assert_eq!(pci::storage_name(0x07), "SAS");
    }

    #[test]
    fn test_storage_name_unassigned() {
        assert_eq!(pci::storage_name(0x0B), "Other");
        assert_eq!(pci::storage_name(0x0C), "Other");
        assert_eq!(pci::storage_name(0x0D), "Other");
    }
}
