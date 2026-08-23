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

fn format_hex(val: u64, nibbles: usize, buf: &mut [u8; 16]) -> &str {
    let mut pos = 0;
    let mut started = false;
    for i in (0..nibbles).rev() {
        let digit = ((val >> (i * 4)) & 0xF) as u8;
        if digit == 0 && !started && i > 0 {
            continue;
        }
        started = true;
        buf[pos] = if digit < 10 { b'0' + digit } else { b'A' + digit - 10 };
        pos += 1;
    }
    if !started {
        buf[pos] = b'0';
        pos += 1;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[..pos]) }
}

pub fn print_hex(val: u64, nibbles: usize) {
    let mut hex_buf = [0u8; 16];
    let s = format_hex(val, nibbles, &mut hex_buf);
    puts(s);
}

fn format_dec(val: u64, buf: &mut [u8; 20]) -> &str {
    if val == 0 {
        buf[0] = b'0';
        return unsafe { core::str::from_utf8_unchecked(&buf[..1]) };
    }
    let mut rev = [0u8; 20];
    let mut n = 0;
    let mut v = val;
    while v > 0 {
        rev[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    let mut j = 0;
    while n > 0 {
        n -= 1;
        buf[j] = rev[n];
        j += 1;
    }
    unsafe { core::str::from_utf8_unchecked(&buf[..j]) }
}

pub fn print_dec(val: u64) {
    let mut dec_buf = [0u8; 20];
    let s = format_dec(val, &mut dec_buf);
    puts(s);
}

/// Print an IPv4 address as `A.B.C.D` to the global print sink.
pub fn print_ip(ip: &[u8; 4]) {
    print_dec(ip[0] as u64);
    putc(b'.');
    print_dec(ip[1] as u64);
    putc(b'.');
    print_dec(ip[2] as u64);
    putc(b'.');
    print_dec(ip[3] as u64);
}

// ── DHCP info formatter (Lua `dhcp` builtin) ───────────────────────────────

const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

#[inline]
fn buf_push(buf: &mut [u8], n: &mut usize, b: u8) {
    if *n < buf.len() {
        buf[*n] = b;
        *n += 1;
    }
}

fn buf_push_str(buf: &mut [u8], n: &mut usize, s: &[u8]) {
    for &b in s {
        buf_push(buf, n, b);
    }
}

fn buf_push_dec(buf: &mut [u8], n: &mut usize, v: u32) {
    let mut tmp = [0u8; 10];
    let mut i = tmp.len();
    let mut x = v;
    loop {
        i -= 1;
        tmp[i] = b'0' + (x % 10) as u8;
        x /= 10;
        if x == 0 {
            break;
        }
    }
    while i < tmp.len() {
        buf_push(buf, n, tmp[i]);
        i += 1;
    }
}

fn buf_push_ip(buf: &mut [u8], n: &mut usize, ip: &[u8; 4]) {
    for i in 0..4 {
        if i > 0 {
            buf_push(buf, n, b'.');
        }
        buf_push_dec(buf, n, ip[i] as u32);
    }
}

fn buf_push_mac(buf: &mut [u8], n: &mut usize, mac: &[u8; 6]) {
    for (i, &b) in mac.iter().enumerate() {
        if i > 0 {
            buf_push(buf, n, b':');
        }
        buf_push(buf, n, HEX_DIGITS[(b >> 4) as usize]);
        buf_push(buf, n, HEX_DIGITS[(b & 0x0F) as usize]);
    }
}

/// Format a DHCP result as `Label: value` lines into `buf` (no trailing newline
/// beyond each line). Used by the host `dhcp_info` callback backing the Lua
/// `dhcp` builtin. `bootfile` may be a null-terminated filename; formatting
/// stops at the first null byte. Returns the number of bytes written.
pub fn format_dhcp_info(
    mac: &[u8; 6],
    ip: &[u8; 4],
    subnet: &[u8; 4],
    gateway: &[u8; 4],
    next_server: &[u8; 4],
    bootfile: &[u8],
    buf: &mut [u8],
) -> usize {
    let mut n = 0usize;
    buf_push_str(buf, &mut n, b"MAC: ");
    buf_push_mac(buf, &mut n, mac);
    buf_push(buf, &mut n, b'\n');
    buf_push_str(buf, &mut n, b"IP: ");
    buf_push_ip(buf, &mut n, ip);
    buf_push(buf, &mut n, b'\n');
    buf_push_str(buf, &mut n, b"Subnet: ");
    buf_push_ip(buf, &mut n, subnet);
    buf_push(buf, &mut n, b'\n');
    buf_push_str(buf, &mut n, b"Gateway: ");
    if gateway == &[0, 0, 0, 0] {
        buf_push_str(buf, &mut n, b"(none)");
    } else {
        buf_push_ip(buf, &mut n, gateway);
    }
    buf_push(buf, &mut n, b'\n');
    buf_push_str(buf, &mut n, b"TFTP Server: ");
    buf_push_ip(buf, &mut n, next_server);
    buf_push(buf, &mut n, b'\n');
    buf_push_str(buf, &mut n, b"Bootfile: ");
    for &b in bootfile {
        if b == 0 {
            break;
        }
        buf_push(buf, &mut n, b);
    }
    buf_push(buf, &mut n, b'\n');
    n
}

pub fn print_fmt(args: fmt::Arguments<'_>) {
    let _ = UartWriter.write_fmt(args);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_format_hex(val: u64, nibbles: usize, expected: &str) {
        let mut buf = [0u8; 16];
        let s = format_hex(val, nibbles, &mut buf);
        assert_eq!(s, expected);
    }

    #[test]
    fn hex_zero() {
        test_format_hex(0, 4, "0");
    }

    #[test]
    fn hex_no_leading_zeros() {
        test_format_hex(0x00FF, 8, "FF");
    }

    #[test]
    fn hex_all_digits() {
        test_format_hex(0xABCD, 4, "ABCD");
    }

    #[test]
    fn hex_lower_nibble() {
        test_format_hex(0x5, 4, "5");
    }

    #[test]
    fn hex_full_width() {
        test_format_hex(0xDEAD_BEEF_CAFE_BABE, 16, "DEADBEEFCAFEBABE");
    }

    #[test]
    fn hex_alternating() {
        test_format_hex(0xAAAA, 4, "AAAA");
    }

    fn test_format_dec(val: u64, expected: &str) {
        let mut buf = [0u8; 20];
        let s = format_dec(val, &mut buf);
        assert_eq!(s, expected);
    }

    #[test]
    fn dec_zero() {
        test_format_dec(0, "0");
    }

    #[test]
    fn dec_single_digit() {
        test_format_dec(7, "7");
    }

    #[test]
    fn dec_round_number() {
        test_format_dec(1000, "1000");
    }

    #[test]
    fn dec_big() {
        test_format_dec(123456789, "123456789");
    }

    #[test]
    fn dec_max_u64() {
        test_format_dec(u64::MAX, "18446744073709551615");
    }

    #[test]
    fn dec_power_of_ten() {
        test_format_dec(1_000_000_000_000_000_000, "1000000000000000000");
    }

    #[test]
    fn dec_million() {
        test_format_dec(1_000_000, "1000000");
    }

    #[test]
    fn hex_single_nibble() {
        test_format_hex(0xF, 1, "F");
    }

    #[test]
    fn hex_zero_nibble_all() {
        test_format_hex(0, 1, "0");
    }

    #[test]
    fn hex_zero_with_nibbles_8() {
        test_format_hex(0, 8, "0");
    }

    #[test]
    fn hex_full_64bit_zero() {
        test_format_hex(0, 16, "0");
    }

    #[test]
    fn hex_truncated_by_nibbles() {
        // Only bottom 4 nibbles shown
        test_format_hex(0x1ABCD, 4, "ABCD");
    }

    #[test]
    fn hex_large_64bit() {
        test_format_hex(0xFEDCBA9876543210, 16, "FEDCBA9876543210");
    }

    #[test]
    fn dec_power_of_two() {
        let mut buf = [0u8; 20];
        let s = format_dec(1 << 20, &mut buf);
        assert_eq!(s, "1048576");
    }

    #[test]
    fn dhcp_info_format() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let ip = [10, 0, 0, 15];
        let subnet = [255, 255, 255, 0];
        let gw = [10, 0, 0, 1];
        let srv = [10, 0, 0, 1];
        // The bootfile is null-terminated; formatting stops at the null byte.
        let bootfile = b"test.lua\0junk";
        let mut buf = [0u8; 384];
        let n = format_dhcp_info(&mac, &ip, &subnet, &gw, &srv, bootfile, &mut buf);
        assert_eq!(
            core::str::from_utf8(&buf[..n]).unwrap(),
            "MAC: 52:54:00:12:34:56\nIP: 10.0.0.15\nSubnet: 255.255.255.0\n\
             Gateway: 10.0.0.1\nTFTP Server: 10.0.0.1\nBootfile: test.lua\n"
        );
        // A zero gateway prints (none).
        let no_gw = [0, 0, 0, 0];
        let n3 = format_dhcp_info(&mac, &ip, &subnet, &no_gw, &srv, bootfile, &mut buf);
        assert!(core::str::from_utf8(&buf[..n3]).unwrap().contains("Gateway: (none)"));
        // Output is bounded by the buffer size.
        let mut small = [0u8; 10];
        let n2 = format_dhcp_info(&mac, &ip, &subnet, &gw, &srv, bootfile, &mut small);
        assert_eq!(n2, 10);
    }

    #[test]
    fn dec_ten() {
        let mut buf = [0u8; 20];
        let s = format_dec(10, &mut buf);
        assert_eq!(s, "10");
    }

    #[test]
    fn dec_hundred() {
        let mut buf = [0u8; 20];
        let s = format_dec(100, &mut buf);
        assert_eq!(s, "100");
    }
}
