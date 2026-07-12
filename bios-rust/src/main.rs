#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(dead_code))]

mod serial;
mod vga;
mod kbd;
mod pci;
#[cfg(not(test))]
mod net;

#[cfg(not(test))]
use core::panic::PanicInfo;

#[cfg(not(test))]
use common::menu::{show_menu, MenuAction};
#[cfg(not(test))]
use common::print;
#[cfg(not(test))]
use common::scan;

#[cfg(not(test))]
fn dual_putc(c: u8) {
    serial::putc(c);
    vga::putc(c);
}

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn _start(_boot_drive: u32) -> ! {
    print::init(dual_putc);
    print::puts("\nRustrapper BIOS Stage2 (Rust)\n");
    pci::pci_print_all();
    match show_menu(common::print::puts, common::print::putc, kbd::getc) {
        MenuAction::StorageScan => {
            print::puts("\nStorage devices:\n");
            scan::scan_devices(pci::detect_device);
        }
        MenuAction::NetworkBoot => {
            print::puts("\n");
            net::scan_network();
        }
    }
    print::puts("Halting.\n");
    serial::flush();
    loop {}
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
