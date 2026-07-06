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

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::String;
    use std::sync::Mutex;
    use std::vec::Vec;

    static CAPTURE_BUF: Mutex<Vec<u8>> = Mutex::new(Vec::new());

    fn capture_putc(c: u8) {
        CAPTURE_BUF.lock().unwrap().push(c);
    }

    fn capture_reset() {
        CAPTURE_BUF.lock().unwrap().clear();
    }

    fn capture_get() -> String {
        String::from_utf8(CAPTURE_BUF.lock().unwrap().clone()).unwrap()
    }

    #[test]
    fn test_device_info_new_defaults() {
        let info = DeviceInfo::new();
        assert_eq!(info.index, 0);
        assert!(!info.present);
        assert!(!info.removable);
        assert_eq!(info.block_count, 0);
        assert_eq!(info.block_size, 512);
        assert!(info.description.is_none());
    }

    #[test]
    fn test_device_info_construct() {
        let mut info = DeviceInfo::new();
        info.index = 1;
        info.present = true;
        info.removable = false;
        info.block_count = 1000;
        info.block_size = 4096;
        info.description = Some("NVMe");
        assert_eq!(info.index, 1);
        assert!(info.present);
        assert!(!info.removable);
        assert_eq!(info.block_count, 1000);
        assert_eq!(info.block_size, 4096);
        assert_eq!(info.description, Some("NVMe"));
    }

    #[test]
    fn test_max_devices_value() {
        assert_eq!(MAX_DEVICES, 32);
    }

    #[test]
    fn test_scan_devices_none() {
        crate::print::init(capture_putc);
        capture_reset();

        fn no_devices(_i: usize, _info: &mut DeviceInfo) -> bool {
            false
        }
        scan_devices(no_devices);
        let out = capture_get();
        assert!(out.contains("Scanning for devices..."));
        assert!(out.contains("Found 0 device(s)"));
    }

    #[test]
    fn test_scan_devices_one_present() {
        crate::print::init(capture_putc);
        capture_reset();

        fn one_device(i: usize, info: &mut DeviceInfo) -> bool {
            if i == 0 {
                info.index = 2;
                info.present = true;
                info.removable = false;
                info.block_count = 500;
                info.block_size = 512;
                info.description = Some("SATA");
                return true;
            }
            false
        }
        scan_devices(one_device);
        let out = capture_get();
        assert!(out.contains("Found 1 device(s)"));
        assert!(out.contains("SATA"));
        assert!(out.contains("500"));
    }

    #[test]
    fn test_scan_devices_multi() {
        crate::print::init(capture_putc);
        capture_reset();

        fn three_devices(i: usize, info: &mut DeviceInfo) -> bool {
            match i {
                0 | 2 | 5 => {
                    info.index = i as u8;
                    info.present = true;
                    info.block_size = 512;
                    info.block_count = 100 + i as u64;
                    if i == 0 { info.description = Some("SATA"); }
                    if i == 2 { info.description = Some("NVMe"); }
                    if i == 5 { info.description = Some("USB"); }
                    true
                }
                _ => false,
            }
        }
        scan_devices(three_devices);
        let out = capture_get();
        assert!(out.contains("Found 3 device(s)"));
    }

    #[test]
    fn test_scan_devices_not_present_skipped() {
        crate::print::init(capture_putc);
        capture_reset();

        fn not_present(i: usize, info: &mut DeviceInfo) -> bool {
            if i == 0 {
                info.present = false;
                return true;
            }
            false
        }
        scan_devices(not_present);
        let out = capture_get();
        assert!(out.contains("Found 0 device(s)"));
    }
}
