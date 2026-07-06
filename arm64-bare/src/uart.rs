use core::ptr::{read_volatile, write_volatile};

const UART_BASE: u64 = 0x0900_0000;

const UART_DR: *mut u32 = (UART_BASE + 0x000) as *mut u32;
const UART_FR: *mut u32 = (UART_BASE + 0x018) as *mut u32;
const UART_FR_TXFF: u32 = 1 << 5;

pub fn putc(c: u8) {
    if c == b'\n' {
        while unsafe { read_volatile(UART_FR) } & UART_FR_TXFF != 0 {}
        unsafe { write_volatile(UART_DR, b'\r' as u32) };
    }
    while unsafe { read_volatile(UART_FR) } & UART_FR_TXFF != 0 {}
    unsafe { write_volatile(UART_DR, c as u32) };
}
