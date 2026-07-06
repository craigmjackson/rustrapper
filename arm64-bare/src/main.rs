#![no_std]
#![no_main]

mod uart;
mod pci;

use core::panic::PanicInfo;

use common::print;
use common::scan;

core::arch::global_asm!(
    ".section .text.boot",
    ".globl _start",
    "_start:",
    "  ldr x30, =_stack_top",
    "  mov sp, x30",
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

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[no_mangle]
pub extern "C" fn main() -> ! {
    print::init(uart::putc);
    print::puts("\nStrapper ARM64 Bare-Metal\n");
    pci::pci_print_all();
    print::puts("\nStorage devices:\n");
    scan::scan_devices(pci::detect_device);
    print::puts("Halting.\n");
    loop {
        unsafe { core::arch::asm!("wfi") }
    }
}
