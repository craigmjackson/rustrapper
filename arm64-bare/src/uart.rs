use core::ptr::{read_volatile, write_volatile};

const UART_BASE: u64 = 0x0900_0000;

const UART_DR: *mut u32 = (UART_BASE + 0x000) as *mut u32;
const UART_FR: *mut u32 = (UART_BASE + 0x018) as *mut u32;
const UART_FR_TXFF: u32 = 1 << 5;
const UART_FR_RXFE: u32 = 1 << 4;

pub fn putc(c: u8) {
    if c == b'\n' {
        while unsafe { read_volatile(UART_FR) } & UART_FR_TXFF != 0 {}
        unsafe { write_volatile(UART_DR, b'\r' as u32) };
    }
    while unsafe { read_volatile(UART_FR) } & UART_FR_TXFF != 0 {}
    unsafe { write_volatile(UART_DR, c as u32) };
}

/// Decodes PL011 escape sequences (arrow keys / home / end / delete) into the
/// [`lua::repl`] key sentinels.
static mut ESC: lua::repl::EscSeq = lua::repl::EscSeq::new();

pub fn getc() -> Option<u8> {
    if unsafe { read_volatile(UART_FR) } & UART_FR_RXFE != 0 {
        return None;
    }
    let c = unsafe { read_volatile(UART_DR) } as u8;
    let esc = unsafe { &mut *core::ptr::addr_of_mut!(ESC) };
    esc.feed(c)
}
