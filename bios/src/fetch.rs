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
    if name.starts_with("http://") { return None; }
    let name = name.strip_prefix("tftp://").unwrap_or(name);
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
    if name.starts_with("http://") { return None; }
    let name = name.strip_prefix("tftp://").unwrap_or(name);
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

// ── io library support (read-only pseudo-filesystem) ────────────────────────
static mut IO_OPEN: [bool; MAX_FETCH_FILES] = [false; MAX_FETCH_FILES];
static mut IO_POS: [usize; MAX_FETCH_FILES] = [0; MAX_FETCH_FILES];
static mut IO_CLOSED: [bool; MAX_FETCH_FILES] = [false; MAX_FETCH_FILES];

fn find_fetched_slot(name: &str) -> Option<usize> {
    unsafe {
        for i in 0..MAX_FETCH_FILES {
            if FETCH_FILES[i].used {
                let nlen = FETCH_FILES[i].name.iter().position(|&b| b == 0).unwrap_or(64);
                if &FETCH_FILES[i].name[..nlen] == name.as_bytes() {
                    return Some(i);
                }
            }
        }
    }
    None
}

pub fn io_open(name: &str, mode: &str) -> Option<u8> {
    if !mode.contains('r') { return None; }
    let slot = find_fetched_slot(name)?;
    unsafe {
        IO_POS[slot] = 0;
        IO_CLOSED[slot] = false;
        IO_OPEN[slot] = true;
    }
    Some(slot as u8)
}

pub fn io_close(id: u8) -> bool {
    let i = id as usize;
    if i >= MAX_FETCH_FILES { return false; }
    unsafe {
        if !IO_OPEN[i] { return false; }
        IO_CLOSED[i] = true;
        IO_OPEN[i] = false;
        true
    }
}

pub fn io_read(id: u8, fmt: u8, n: usize, buf: &mut [u8]) -> Option<usize> {
    let i = id as usize;
    if i >= MAX_FETCH_FILES { return None; }
    unsafe {
        if !IO_OPEN[i] || IO_CLOSED[i] { return None; }
        let base = FETCH_FILES[i].base;
        let len = FETCH_FILES[i].len;
        let pos = IO_POS[i];
        if pos >= len { return None; }
        match fmt {
            0 => {
                let nr = (len - pos).min(buf.len());
                if nr == 0 { return None; }
                let src = (base + pos) as *const u8;
                core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), nr);
                IO_POS[i] = pos + nr;
                Some(nr)
            }
            1 => {
                let mut end = pos;
                while end < len && *((base + end) as *const u8) != b'\n' { end += 1; }
                let nr = (end - pos).min(buf.len());
                if nr == 0 && pos >= len { return None; }
                if nr > 0 {
                    let src = (base + pos) as *const u8;
                    core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), nr);
                }
                IO_POS[i] = if end < len { end + 1 } else { end };
                if nr == 0 { None } else { Some(nr) }
            }
            _ => {
                let want = if n == 0 { buf.len() } else { n };
                let nr = (len - pos).min(want).min(buf.len());
                if nr == 0 { return None; }
                let src = (base + pos) as *const u8;
                core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), nr);
                IO_POS[i] = pos + nr;
                Some(nr)
            }
        }
    }
}

pub fn io_write(_id: u8, _data: &[u8]) -> bool { false }
pub fn io_flush(_id: u8) -> bool { true }
pub fn io_seek(_id: u8, _whence: &str, _offset: i64) -> Option<usize> { None }

pub fn io_type(id: u8) -> Option<&'static str> {
    let i = id as usize;
    if i >= MAX_FETCH_FILES { return None; }
    unsafe {
        if !IO_OPEN[i] { return None; }
        if IO_CLOSED[i] { Some("closed file") } else { Some("file") }
    }
}

pub fn io_size(_id: u8) -> usize { 0 }

pub fn set_io_callbacks(state: &mut lua::LuaState) {
    state.set_io_open(Some(io_open));
    state.set_io_close(Some(io_close));
    state.set_io_read(Some(io_read));
    state.set_io_write(Some(io_write));
    state.set_io_flush(Some(io_flush));
    state.set_io_type(Some(io_type));
    state.set_io_size(Some(io_size));
    state.set_io_seek(Some(io_seek));
}
