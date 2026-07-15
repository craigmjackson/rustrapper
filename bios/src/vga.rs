use core::ptr::{read_volatile, write_volatile};

const VGA_BASE: usize = 0xB8000;
const COLS: usize = 80;
const ROWS: usize = 25;
static mut COL: usize = 0;
static mut ROW: usize = 0;

fn scroll_up() {
    unsafe {
        for i in 0..(ROWS - 1) * COLS * 2 {
            let v = read_volatile((VGA_BASE + i + COLS * 2) as *const u8);
            write_volatile((VGA_BASE + i) as *mut u8, v);
        }
        for i in 0..COLS * 2 {
            let off = if i % 2 == 0 { 0x20 } else { 0x07 };
            write_volatile((VGA_BASE + (ROWS - 1) * COLS * 2 + i) as *mut u8, off);
        }
    }
}

fn put_char(c: u8) {
    unsafe {
        if c == b'\r' {
            COL = 0;
            return;
        }
        if c == b'\n' {
            COL = 0;
            ROW += 1;
            if ROW >= ROWS {
                ROW = ROWS - 1;
                scroll_up();
            }
            return;
        }
        let idx = (ROW * COLS + COL) * 2;
        write_volatile((VGA_BASE + idx) as *mut u8, c);
        write_volatile((VGA_BASE + idx + 1) as *mut u8, 0x07);
        COL += 1;
        if COL >= COLS {
            COL = 0;
            ROW += 1;
            if ROW >= ROWS {
                ROW = ROWS - 1;
                scroll_up();
            }
        }
    }
}

pub fn putc(c: u8) {
    if c == b'\n' {
        put_char(b'\r');
    }
    put_char(c);
}
