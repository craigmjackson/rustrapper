//! UEFI `fetch()` builtin support.
//!
//! The Lua `fetch("file")` builtin downloads named files from the DHCP TFTP
//! server (`next_server`) into host memory, so a PXE Lua script can pull
//! multiple files (e.g. a kernel and an initrd) and hand their byte counts
//! back to the script. Each successful `fetch` fills one slot of a fixed-size
//! table; all buffers come from a single lazily-allocated AllocatePool region
//! (16 MB split into per-file windows).

use core::ffi::c_void;
use crate::efi::*;
use common::tftp::TftpSink;

const BOOT_SVC_ALLOCATE_POOL: usize = 0x40;
const EFI_LOADER_DATA: u32 = 2;

type AllocatePoolFn = unsafe extern "efiapi" fn(
    pool_type: u32,
    size: u64,
    buffer: *mut *mut c_void,
) -> EFI_STATUS;

fn read_boot_svc_fn<T>(gbs: *const c_void, offset: usize) -> T {
    let ptr = (gbs as usize + offset) as *const *const c_void;
    unsafe { core::mem::transmute_copy(&*ptr) }
}

/// Maximum number of files a script can fetch in one run.
pub const MAX_FETCH_FILES: usize = 4;
/// Total memory reserved for all fetched files.
pub const FETCH_TOTAL_CAP: usize = 16 * 1024 * 1024;
/// Per-file capacity inside the shared buffer.
const PER_FILE_CAP: usize = FETCH_TOTAL_CAP / MAX_FETCH_FILES;

/// One downloaded file.
#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct FetchFile {
    pub name: [u8; 64],
    pub base: *mut u8,
    pub len: usize,
    pub used: bool,
}

/// Network context needed to run a TFTP transfer, set by [`set_context`]
/// right before a Lua script executes. `fetch` uses direct MMIO e1000. Also
/// keeps the negotiated network details for the Lua `dhcp` builtin.
struct FetchContext {
    system_table: *const EFI_SYSTEM_TABLE,
    base: u64,
    mac: [u8; 6],
    src_ip: [u8; 4],
    server_ip: [u8; 4],
    subnet: [u8; 4],
    gateway: [u8; 4],
    bootfile: [u8; 128],
}

static mut FETCH_CTX: FetchContext = FetchContext {
    system_table: core::ptr::null(),
    base: 0,
    mac: [0; 6],
    src_ip: [0; 4],
    server_ip: [0; 4],
    subnet: [0; 4],
    gateway: [0; 4],
    bootfile: [0; 128],
};

static mut FETCH_FILES: [FetchFile; MAX_FETCH_FILES] = [
    FetchFile { name: [0; 64], base: core::ptr::null_mut(), len: 0, used: false },
    FetchFile { name: [0; 64], base: core::ptr::null_mut(), len: 0, used: false },
    FetchFile { name: [0; 64], base: core::ptr::null_mut(), len: 0, used: false },
    FetchFile { name: [0; 64], base: core::ptr::null_mut(), len: 0, used: false },
];

/// The shared buffer all fetched files are written into, allocated lazily.
static mut FETCH_BUFFER: *mut u8 = core::ptr::null_mut();

/// Record the network context for the upcoming Lua run and reset the slots.
pub fn set_context(st: &EFI_SYSTEM_TABLE, base: u64, mac: &[u8; 6], cfg: &common::dhcp::DhcpConfig) {
    unsafe {
        FETCH_CTX = FetchContext {
            system_table: st,
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

/// TFTP sink writing into one slot's window of the shared buffer.
struct FetchSink {
    base: *mut u8,
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
            core::ptr::copy_nonoverlapping(data.as_ptr(), self.base.add(self.offset), data.len());
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
/// in a slot, and return the byte count (or `None` on failure).
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

    let (st, base, mac, src_ip, server_ip) = unsafe {
        (
            FETCH_CTX.system_table,
            FETCH_CTX.base,
            FETCH_CTX.mac,
            FETCH_CTX.src_ip,
            FETCH_CTX.server_ip,
        )
    };
    if st.is_null() || base == 0 || server_ip == [0; 4] {
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

    // Lazily allocate the shared buffer once.
    unsafe {
        if FETCH_BUFFER.is_null() {
            let gbs = (*st).boot_services;
            let allocate_pool: AllocatePoolFn = read_boot_svc_fn(gbs, BOOT_SVC_ALLOCATE_POOL);
            let mut buf: *mut c_void = core::ptr::null_mut();
            let status = allocate_pool(EFI_LOADER_DATA, FETCH_TOTAL_CAP as u64, &mut buf);
            if status != EFI_SUCCESS {
                return None;
            }
            FETCH_BUFFER = buf as *mut u8;
        }
    }

    let slot_base = unsafe { FETCH_BUFFER.add(idx * PER_FILE_CAP) };
    let mut sink = FetchSink {
        base: slot_base,
        offset: 0,
        capacity: PER_FILE_CAP,
    };

    let e1000 = crate::net::DirectMmioE1000::new(base);
    let con_out = unsafe { &*(*st).con_out };
    let size =
        crate::net::tftp_download_e1000(con_out, &e1000, &mac, &src_ip, &server_ip, name, &mut sink)?;

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
    let (st, base, mac, src_ip, server_ip) = unsafe {
        (
            FETCH_CTX.system_table,
            FETCH_CTX.base,
            FETCH_CTX.mac,
            FETCH_CTX.src_ip,
            FETCH_CTX.server_ip,
        )
    };
    if st.is_null() || base == 0 || server_ip == [0; 4] {
        return None;
    }
    let mut sink = LoadSink { buf, offset: 0 };
    let e1000 = crate::net::DirectMmioE1000::new(base);
    let con_out = unsafe { &*(*st).con_out };
    crate::net::tftp_download_e1000(con_out, &e1000, &mac, &src_ip, &server_ip, name, &mut sink)
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
                core::ptr::copy_nonoverlapping(base.add(pos), buf.as_mut_ptr(), nr);
                IO_POS[i] = pos + nr;
                Some(nr)
            }
            1 => {
                let mut end = pos;
                while end < len && *base.add(end) != b'\n' { end += 1; }
                let nr = (end - pos).min(buf.len());
                if nr == 0 && pos >= len { return None; }
                if nr > 0 {
                    core::ptr::copy_nonoverlapping(base.add(pos), buf.as_mut_ptr(), nr);
                }
                IO_POS[i] = if end < len { end + 1 } else { end };
                if nr == 0 { None } else { Some(nr) }
            }
            _ => {
                let want = if n == 0 { buf.len() } else { n };
                let nr = (len - pos).min(want).min(buf.len());
                if nr == 0 { return None; }
                core::ptr::copy_nonoverlapping(base.add(pos), buf.as_mut_ptr(), nr);
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


pub fn os_remove(name: &str) -> bool {
    // Remove a fetched file from the pseudo-filesystem by name.
    unsafe {
        for i in 0..MAX_FETCH_FILES {
            if FETCH_FILES[i].used {
                let nlen = FETCH_FILES[i].name.iter().position(|&b| b == 0).unwrap_or(64);
                if &FETCH_FILES[i].name[..nlen] == name.as_bytes() {
                    FETCH_FILES[i].used = false;
                    return true;
                }
            }
        }
    }
    false
}

pub fn os_rename(old: &str, new: &str) -> bool {
    unsafe {
        for i in 0..MAX_FETCH_FILES {
            if FETCH_FILES[i].used {
                let nlen = FETCH_FILES[i].name.iter().position(|&b| b == 0).unwrap_or(64);
                if &FETCH_FILES[i].name[..nlen] == old.as_bytes() {
                    let nb = new.as_bytes();
                    if nb.len() >= 64 { return false; }
                    FETCH_FILES[i].name = [0; 64];
                    FETCH_FILES[i].name[..nb.len()].copy_from_slice(nb);
                    return true;
                }
            }
        }
    }
    false
}

pub fn set_io_callbacks(state: &mut lua::LuaState) {
    state.set_io_open(Some(io_open));
    state.set_io_close(Some(io_close));
    state.set_io_read(Some(io_read));
    state.set_io_write(Some(io_write));
    state.set_io_flush(Some(io_flush));
    state.set_io_type(Some(io_type));
    state.set_io_size(Some(io_size));
    state.set_io_seek(Some(io_seek));
    state.set_os_remove(Some(os_remove));
    state.set_os_rename(Some(os_rename));
}
