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
/// right before a Lua script executes. `fetch` uses direct MMIO e1000.
struct FetchContext {
    system_table: *const EFI_SYSTEM_TABLE,
    base: u64,
    mac: [u8; 6],
    src_ip: [u8; 4],
    server_ip: [u8; 4],
}

static mut FETCH_CTX: FetchContext = FetchContext {
    system_table: core::ptr::null(),
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
