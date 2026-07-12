const SET1_MAP: [u8; 0x3A] = [
    0,    // 0x00
    0,    // 0x01 Escape
    b'1', // 0x02
    b'2', // 0x03
    b'3', // 0x04
    b'4', // 0x05
    b'5', // 0x06
    b'6', // 0x07
    b'7', // 0x08
    b'8', // 0x09
    b'9', // 0x0A
    b'0', // 0x0B
    b'-', // 0x0C
    b'=', // 0x0D
    0,    // 0x0E Backspace (handled separately)
    0,    // 0x0F Tab
    b'q', // 0x10
    b'w', // 0x11
    b'e', // 0x12
    b'r', // 0x13
    b't', // 0x14
    b'y', // 0x15
    b'u', // 0x16
    b'i', // 0x17
    b'o', // 0x18
    b'p', // 0x19
    b'[', // 0x1A
    b']', // 0x1B
    0,    // 0x1C Enter (handled separately)
    0,    // 0x1D Ctrl
    b'a', // 0x1E
    b's', // 0x1F
    b'd', // 0x20
    b'f', // 0x21
    b'g', // 0x22
    b'h', // 0x23
    b'j', // 0x24
    b'k', // 0x25
    b'l', // 0x26
    b';', // 0x27
    b'\'',// 0x28
    b'`', // 0x29
    0,    // 0x2A Shift
    b'\\',// 0x2B
    b'z', // 0x2C
    b'x', // 0x2D
    b'c', // 0x2E
    b'v', // 0x2F
    b'b', // 0x30
    b'n', // 0x31
    b'm', // 0x32
    b',', // 0x33
    b'.', // 0x34
    b'/', // 0x35
    0,    // 0x36 Shift
    b'*', // 0x37
    0,    // 0x38 Alt
    b' ', // 0x39
];

pub fn getc() -> Option<u8> {
    unsafe {
        let status: u8;
        core::arch::asm!("in al, dx", in("dx") 0x64u16, out("al") status);
        if status & 0x01 == 0 {
            return None;
        }
        let scancode: u8;
        core::arch::asm!("in al, dx", in("dx") 0x60u16, out("al") scancode);
        if scancode & 0x80 != 0 {
            return None;
        }
        if scancode == 0x1C {
            return Some(b'\n');
        }
        if scancode == 0x0E {
            return Some(0x08);
        }
        if (scancode as usize) < SET1_MAP.len() {
            let c = SET1_MAP[scancode as usize];
            if c != 0 {
                return Some(c);
            }
        }
        None
    }
}
