//! Lua `fetch()` / `dhcp` host callbacks for the native target.
//!
//! Mirrors the firmware targets' `fetch.rs`: the `dhcp` builtin records the
//! network context (discovered from the kernel) and enables `fetch_file`, which
//! downloads named files from the TFTP server over a std UDP socket. Downloads
//! are kept in memory in a Vec (no fixed slots needed on a std host).

use std::net::Ipv4Addr;
use std::sync::Mutex;

use common::tftp::TftpSink;

/// (TFTP server, local IP) recorded by the `dhcp` builtin.
static CTX: Mutex<(Ipv4Addr, Ipv4Addr)> =
    Mutex::new((Ipv4Addr::UNSPECIFIED, Ipv4Addr::UNSPECIFIED));
/// Files fetched this session, kept alive in host memory.
static FILES: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());

/// Record the network context for `fetch()` (called by `net::setup_fetch_context`).
pub fn set_context(server: Ipv4Addr, local: Ipv4Addr) {
    *CTX.lock().unwrap() = (server, local);
    FILES.lock().unwrap().clear();
}

/// The recorded (TFTP server, local IP) addresses.
pub fn context() -> (Ipv4Addr, Ipv4Addr) {
    *CTX.lock().unwrap()
}

struct FileSink<'a> {
    data: &'a mut Vec<u8>,
}

impl TftpSink for FileSink<'_> {
    fn write_block(&mut self, data: &[u8]) -> Result<(), ()> {
        self.data.extend_from_slice(data);
        Ok(())
    }
    fn finalize(&mut self, _size: usize) -> Result<(), ()> {
        Ok(())
    }
}

/// Host `fetch()` callback: download `name` from the TFTP server, keep it in
/// memory, and return its byte count (or `None` on failure).
pub fn fetch_file(name: &str) -> Option<usize> {
    let server = CTX.lock().unwrap().0;
    if server == Ipv4Addr::UNSPECIFIED {
        return None;
    }
    let mut data = Vec::new();
    {
        let mut sink = FileSink { data: &mut data };
        crate::net::tftp_download(server, name, &mut sink)?;
    }
    FILES.lock().unwrap().push((name.to_string(), data.clone()));
    Some(data.len())
}

/// Host `dofile()` callback: download `name` from the TFTP server into the
/// interpreter's buffer and return its length (or `None` on failure).
pub fn load_file(name: &str, buf: &mut [u8]) -> Option<usize> {
    let server = CTX.lock().unwrap().0;
    if server == Ipv4Addr::UNSPECIFIED {
        return None;
    }
    let mut data = Vec::new();
    {
        let mut sink = FileSink { data: &mut data };
        crate::net::tftp_download(server, name, &mut sink)?;
    }
    if data.len() > buf.len() {
        return None;
    }
    buf[..data.len()].copy_from_slice(&data);
    Some(data.len())
}
