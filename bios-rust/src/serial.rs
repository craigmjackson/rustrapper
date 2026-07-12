pub fn putc(c: u8) {
    if c == b'\n' {
        putc_raw(b'\r');
    }
    putc_raw(c);
}

fn putc_raw(c: u8) {
    unsafe {
        loop {
            let lsr: u8;
            core::arch::asm!("in al, dx", in("dx") 0x3FDu16, out("al") lsr);
            if lsr & 0x20 != 0 {
                break;
            }
        }
        core::arch::asm!("out dx, al", in("dx") 0x3F8u16, in("al") c);
    }
}

pub fn flush() {
    unsafe {
        loop {
            let lsr: u8;
            core::arch::asm!("in al, dx", in("dx") 0x3FDu16, out("al") lsr);
            if lsr & 0x40 != 0 {
                break;
            }
        }
    }
}
