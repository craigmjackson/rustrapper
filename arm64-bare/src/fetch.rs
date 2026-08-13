//! ARM64 bare-metal `fetch()` builtin support.
//!
//! Mirrors `uefi/src/fetch.rs`: a PXE Lua script can pull multiple files from
//! the DHCP TFTP server via `fetch("file")`. Files land in a static BSS buffer
//! (no heap on bare-metal) split into per-file windows; each successful fetch
//! returns its byte count to the script.

use common::dhcp::DhcpConfig;
use common::tftp::TftpSink;

/// Maximum number of files a script can fetch in one run.
pub const MAX_FETCH_FILES: usize = 4;
/// Total memory reserved for all fetched files (16 MB of BSS).
pub const FETCH_TOTAL_CAP: usize = 16 * 1024 * 1024;
/// Per-file capacity inside the shared buffer.
const PER_FILE_CAP: usize = FETCH_TOTAL_CAP / MAX_FETCH_FILES;

/// One downloaded file.
#[allow(dead_code)]
pub struct FetchFile {
    pub name: [u8; 64],
    pub base: *mut u8,
    pub len: usize,
    pub used: bool,
}

/// Network context needed to run a TFTP transfer, set by [`set_context`]
/// right before a Lua script executes.
struct FetchContext {
    base: u64,
    mac: [u8; 6],
    src_ip: [u8; 4],
    server_ip: [u8; 4],
}

static mut FETCH_CTX: FetchContext = FetchContext {
    base: 0,
    mac: [0; 6],
    src_ip: [0; 4],
    server_ip: [0; 4],
};

static mut FETCH_FILES: [FetchFile; MAX_FETCH_FILES] = [
    FetchFile { name: [0; 64], base: core::ptr::null_mut(), len: 0, used: false },
    FetchFile { name: [0; 64], base: core::ptr::null_mut(), len: 0, used: false },
    FetchFile { name: [0; 64], base: core::ptr::null_mut(), len: 0, used: false },
    FetchFile { name: [0; 64], base: core::ptr::null_mut(), len: 0, used: false },
];

/// Shared buffer all fetched files are written into (BSS, zeroed at boot).
static mut FETCH_BUFFER: [u8; FETCH_TOTAL_CAP] = [0; FETCH_TOTAL_CAP];

/// Record the network context for the upcoming Lua run and reset the slots.
pub fn set_context(base: u64, mac: &[u8; 6], cfg: &DhcpConfig) {
    unsafe {
        FETCH_CTX = FetchContext {
            base,
            mac: *mac,
            src_ip: cfg.yiaddr,
            server_ip: cfg.next_server,
        };
        for f in 0..MAX_FETCH_FILES {
            FETCH_FILES[f].used = false;
        }
    }
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

/// Host `fetch()` callback: download `name` from the TFTP server, record it
/// in a slot, and return the byte count (or `None` on failure).
pub fn fetch_file(name: &str) -> Option<usize> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() >= 64 {
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

    let buffer_base = &raw mut FETCH_BUFFER as *mut u8;
    let slot_base = unsafe { buffer_base.add(idx * PER_FILE_CAP) };
    let mut sink = FetchSink {
        base: slot_base,
        offset: 0,
        capacity: PER_FILE_CAP,
    };

    let size = crate::net::tftp_download(base, &mac, &src_ip, &server_ip, name, &mut sink)?;

    unsafe {
        let mut n = [0u8; 64];
        n[..bytes.len()].copy_from_slice(bytes);
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
