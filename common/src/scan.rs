use crate::print::{print_dec, print_hex, putc, puts};

pub const MAX_DEVICES: usize = 32;

#[derive(Clone, Copy)]
pub struct DeviceInfo {
    pub index: u8,
    pub present: bool,
    pub removable: bool,
    pub block_count: u64,
    pub block_size: u16,
    pub description: Option<&'static str>,
}

impl DeviceInfo {
    pub const fn new() -> Self {
        Self {
            index: 0,
            present: false,
            removable: false,
            block_count: 0,
            block_size: 512,
            description: None,
        }
    }
}

pub fn scan_devices(detect: fn(usize, &mut DeviceInfo) -> bool) {
    puts("Scanning for devices...\r\n");
    let mut found = 0;
    for i in 0..MAX_DEVICES {
        let mut info = DeviceInfo::new();
        if !detect(i, &mut info) {
            continue;
        }
        if !info.present {
            continue;
        }
        found += 1;
        puts("  Device ");
        print_dec(i as u64);
        puts(": ");
        putc(if info.removable { b'R' } else { b'F' });
        puts(" ");
        if let Some(desc) = info.description {
            puts(desc);
        }
        puts(" (");
        print_hex(info.index as u64, 2);
        puts(") - ");
        print_dec(info.block_count);
        puts(" x ");
        print_dec(info.block_size as u64);
        puts(" byte sectors\r\n");
    }
    puts("Found ");
    print_dec(found as u64);
    puts(" device(s)\r\n");
}
