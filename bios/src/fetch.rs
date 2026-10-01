//! BIOS `fetch()` builtin support.
//!
//! Mirrors `uefi/src/fetch.rs` and `arm64-bare/src/fetch.rs`: a PXE Lua script
//! can pull multiple files from the DHCP TFTP server via `fetch("file")`.
//! Files land in a fixed extended-RAM region (no heap on BIOS) split into
//! per-file windows; each successful fetch returns its byte count to the
//! script.

use common::dhcp::DhcpConfig;
use common::tftp::TftpSink;

/// Maximum number of files a script can fetch in one run.
pub const MAX_FETCH_FILES: usize = 4;
/// Total memory reserved for all fetched files.
pub const FETCH_TOTAL_CAP: usize = 16 * 1024 * 1024;
/// Per-file capacity inside the shared region.
const PER_FILE_CAP: usize = FETCH_TOTAL_CAP / MAX_FETCH_FILES;
/// Fixed base of the fetch region — 32 MB, well above the bootfile sink
/// (2 MB) and its 16 MB window, and within the first 128 MB QEMU provides.
const FETCH_BASE: usize = 0x2000000;

/// One downloaded file.
#[allow(dead_code)]
pub struct FetchFile {
    pub name: [u8; 64],
    pub base: usize,
    pub len: usize,
    pub used: bool,
}

/// Network context needed to run a TFTP transfer, set by [`set_context`]
/// right before a Lua script executes. Also keeps the negotiated network
/// details for the Lua `dhcp` builtin.
struct FetchContext {
    base: u64,
    mac: [u8; 6],
    src_ip: [u8; 4],
    server_ip: [u8; 4],
    subnet: [u8; 4],
    gateway: [u8; 4],
    bootfile: [u8; 128],
}

static mut FETCH_CTX: FetchContext = FetchContext {
    base: 0,
    mac: [0; 6],
    src_ip: [0; 4],
    server_ip: [0; 4],
    subnet: [0; 4],
    gateway: [0; 4],
    bootfile: [0; 128],
};

static mut FETCH_FILES: [FetchFile; MAX_FETCH_FILES] = [
    FetchFile { name: [0; 64], base: 0, len: 0, used: false },
    FetchFile { name: [0; 64], base: 0, len: 0, used: false },
    FetchFile { name: [0; 64], base: 0, len: 0, used: false },
    FetchFile { name: [0; 64], base: 0, len: 0, used: false },
];

/// Record the network context for the upcoming Lua run and reset the slots.
pub fn set_context(base: u64, mac: &[u8; 6], cfg: &DhcpConfig) {
    unsafe {
        FETCH_CTX = FetchContext {
            base,
            mac: *mac,
            src_ip: cfg.yiaddr,
            server_ip: cfg.next_server,
            subnet: cfg.subnet,
            gateway: cfg.gateway,
            bootfile: cfg.bootfile,
        };
        for f in 0..MAX_FETCH_FILES {
            FETCH_FILES[f].used = false;
        }
    }
}

/// Host `dhcp_info` callback for the Lua `dhcp` builtin: format the negotiated
/// MAC / IP / subnet / gateway / TFTP server / bootfile into `buf`.
pub fn dhcp_info(buf: &mut [u8]) -> usize {
    let (mac, src_ip, subnet, gateway, server_ip, bootfile) = unsafe {
        (
            FETCH_CTX.mac,
            FETCH_CTX.src_ip,
            FETCH_CTX.subnet,
            FETCH_CTX.gateway,
            FETCH_CTX.server_ip,
            FETCH_CTX.bootfile,
        )
    };
    common::print::format_dhcp_info(&mac, &src_ip, &subnet, &gateway, &server_ip, &bootfile, buf)
}

/// TFTP sink writing into one slot's window of the fixed fetch region.
struct FetchSink {
    base: usize,
    offset: usize,
    capacity: usize,
}

impl TftpSink for FetchSink {
    fn write_block(&mut self, data: &[u8]) -> Result<(), ()> {
        let new_off = self.offset + data.len();
        if new_off > self.capacity {
            return Err(());
        }
        unsafe {
            let dst = (self.base + self.offset) as *mut u8;
            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        self.offset = new_off;
        Ok(())
    }

    fn finalize(&mut self, _size: usize) -> Result<(), ()> {
        Ok(())
    }
}

/// TFTP sink writing into a caller-provided buffer (for `dofile()`).
struct LoadSink<'a> {
    buf: &'a mut [u8],
    offset: usize,
}

impl TftpSink for LoadSink<'_> {
    fn write_block(&mut self, data: &[u8]) -> Result<(), ()> {
        let new_off = self.offset + data.len();
        if new_off > self.buf.len() {
            return Err(());
        }
        self.buf[self.offset..new_off].copy_from_slice(data);
        self.offset = new_off;
        Ok(())
    }

    fn finalize(&mut self, _size: usize) -> Result<(), ()> {
        Ok(())
    }
}

/// Host `dhcp_values` callback for the Lua `dhcp` builtin: fill the structured
/// DHCP result so scripts can read `mac` / `ip` / `subnet` / `gateway` /
/// `server` / `bootfile` globals.
pub fn dhcp_values(v: &mut lua::DhcpValues) {
    unsafe {
        v.mac = FETCH_CTX.mac;
        v.ip = FETCH_CTX.src_ip;
        v.subnet = FETCH_CTX.subnet;
        v.gateway = FETCH_CTX.gateway;
        v.server = FETCH_CTX.server_ip;
        v.bootfile = FETCH_CTX.bootfile;
        v.tftp_port = 69;
    }
}

/// Host `fetch()` callback: download `name` from the TFTP server, record it
/// in a slot under the local name `save_as`, and return the byte count (or
/// `None` on failure).
pub fn fetch_file(name: &str, save_as: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() >= 64 {
        return None;
    }
    let save = save_as.as_bytes();
    if save.is_empty() || save.len() >= 64 {
        return None;
    }

    let (base, mac, src_ip, server_ip) = unsafe {
        (
            FETCH_CTX.base,
            FETCH_CTX.mac,
            FETCH_CTX.src_ip,
            FETCH_CTX.server_ip,
        )
    };
    if base == 0 || server_ip == [0; 4] {
        return None;
    }

    // Pick the first free slot.
    let mut idx = None;
    for i in 0..MAX_FETCH_FILES {
        if !unsafe { FETCH_FILES[i].used } {
            idx = Some(i);
            break;
        }
    }
    let idx = idx?;

    let slot_base = FETCH_BASE + idx * PER_FILE_CAP;
    let mut sink = FetchSink {
        base: slot_base,
        offset: 0,
        capacity: PER_FILE_CAP,
    };

    let size = crate::net::tftp_download(base, &mac, &src_ip, &server_ip, name, &mut sink)?;

    unsafe {
        let mut n = [0u8; 64];
        n[..save.len()].copy_from_slice(save);
        FETCH_FILES[idx] = FetchFile {
            name: n,
            base: slot_base,
            len: size,
            used: true,
        };
    }
    Some(size)
}

/// Host `dofile()` callback: download `name` from the TFTP server into the
/// interpreter's buffer and return its length (or `None` on failure).
pub fn load_file(name: &str, buf: &mut [u8]) -> Option<usize> {
    let (base, mac, src_ip, server_ip) = unsafe {
        (
            FETCH_CTX.base,
            FETCH_CTX.mac,
            FETCH_CTX.src_ip,
            FETCH_CTX.server_ip,
        )
    };
    if base == 0 || server_ip == [0; 4] {
        return None;
    }
    let mut sink = LoadSink { buf, offset: 0 };
    crate::net::tftp_download(base, &mac, &src_ip, &server_ip, name, &mut sink)
}
