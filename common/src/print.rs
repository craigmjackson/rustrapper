use core::fmt::{self, Write};

pub type PutcFn = fn(u8);

static mut PUTC_FN: Option<PutcFn> = None;

pub fn init(f: PutcFn) {
    unsafe { PUTC_FN = Some(f) }
}

fn putc_raw(c: u8) {
    if let Some(f) = unsafe { PUTC_FN } {
        f(c)
    }
}

pub fn puts(s: &str) {
    for &b in s.as_bytes() {
        putc_raw(b);
    }
}

pub fn putc(c: u8) {
    putc_raw(c);
}

struct UartWriter;

impl Write for UartWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        puts(s);
        Ok(())
    }
}

pub fn print_hex(val: u64, nibbles: usize) {
    let mut started = false;
    for i in (0..nibbles).rev() {
        let digit = ((val >> (i * 4)) & 0xF) as u8;
        if digit == 0 && !started && i > 0 {
            continue;
        }
        started = true;
        putc(if digit < 10 { b'0' + digit } else { b'A' + digit - 10 });
    }
    if !started {
        putc(b'0');
    }
}

pub fn print_dec(val: u64) {
    let mut buf = [0u8; 20];
    let mut i = 0;
    let mut v = val;
    if v == 0 {
        putc(b'0');
        return;
    }
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        putc(buf[i]);
    }
}

pub fn print_fmt(args: fmt::Arguments<'_>) {
    let _ = UartWriter.write_fmt(args);
}


