//! Lua `fetch()` / `dhcp` host callbacks for the native target.
//!
//! Mirrors the firmware targets' `fetch.rs`: the `dhcp` builtin records the
//! network context (discovered from the kernel) and enables `fetch_file`, which
//! downloads named files from the TFTP server over a std UDP socket. Downloads
//! are kept in memory in a Vec (no fixed slots needed on a std host).

use std::net::Ipv4Addr;
use std::sync::Mutex;

use common::tftp::TftpSink;

/// Network details recorded by the `dhcp` builtin (discovered from the kernel
/// rather than a real DHCP response on this std host).
#[derive(Clone, Copy)]
struct NetInfo {
    server: Ipv4Addr,
    local: Ipv4Addr,
    gateway: Ipv4Addr,
    subnet: [u8; 4],
    mac: [u8; 6],
    bootfile: [u8; 128],
    tftp_port: u16,
}

/// Network context recorded by the `dhcp` builtin.
static CTX: Mutex<Option<NetInfo>> = Mutex::new(None);
/// Files fetched this session, kept alive in host memory.
static FILES: Mutex<Vec<(String, Vec<u8>)>> = Mutex::new(Vec::new());

/// Record the network context for `fetch()` (called by `net::setup_fetch_context`).
pub fn set_context(
    server: Ipv4Addr,
    local: Ipv4Addr,
    gateway: Ipv4Addr,
    subnet: [u8; 4],
    mac: [u8; 6],
    bootfile: &[u8],
    tftp_port: u16,
) {
    let mut bf = [0u8; 128];
    let l = bootfile.len().min(127);
    bf[..l].copy_from_slice(&bootfile[..l]);
    *CTX.lock().unwrap() = Some(NetInfo {
        server,
        local,
        gateway,
        subnet,
        mac,
        bootfile: bf,
        tftp_port,
    });
    FILES.lock().unwrap().clear();
}

/// The recorded (TFTP server, local IP) addresses.
pub fn context() -> (Ipv4Addr, Ipv4Addr) {
    let lock = CTX.lock().unwrap();
    match &*lock {
        Some(i) => (i.server, i.local),
        None => (Ipv4Addr::UNSPECIFIED, Ipv4Addr::UNSPECIFIED),
    }
}

/// Host `dhcp_info` callback for the Lua `dhcp` builtin: format the discovered
/// MAC / IP / subnet / gateway / TFTP server / bootfile into `buf`.
pub fn dhcp_info(buf: &mut [u8]) -> usize {
    let lock = CTX.lock().unwrap();
    match &*lock {
        Some(i) => common::print::format_dhcp_info(
            &i.mac,
            &i.local.octets(),
            &i.subnet,
            &i.gateway.octets(),
            &i.server.octets(),
            &i.bootfile,
            buf,
        ),
        None => 0,
    }
}

/// Host `dhcp_values` callback for the Lua `dhcp` builtin: fill the structured
/// DHCP result so scripts can read `mac` / `ip` / `subnet` / `gateway` /
/// `server` / `bootfile` globals.
pub fn dhcp_values(v: &mut lua::DhcpValues) {
    let lock = CTX.lock().unwrap();
    if let Some(i) = &*lock {
        v.mac = i.mac;
        v.ip = i.local.octets();
        v.subnet = i.subnet;
        v.gateway = i.gateway.octets();
        v.server = i.server.octets();
        v.bootfile = i.bootfile;
        v.tftp_port = i.tftp_port;
    }
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
/// memory under the local name `save_as`, and return its byte count (or `None`
/// on failure).
pub fn fetch_file(name: &str, save_as: &str) -> Option<usize> {
    let server = CTX.lock().unwrap().as_ref().map(|i| i.server)?;
    if server == Ipv4Addr::UNSPECIFIED {
        return None;
    }
    let mut data = Vec::new();
    {
        let mut sink = FileSink { data: &mut data };
        crate::net::tftp_download(server, name, &mut sink)?;
    }
    FILES.lock().unwrap().push((save_as.to_string(), data.clone()));
    Some(data.len())
}

/// Host `dofile()` callback: download `name` from the TFTP server into the
/// interpreter's buffer and return its length (or `None` on failure).
pub fn load_file(name: &str, buf: &mut [u8]) -> Option<usize> {
    let server = CTX.lock().unwrap().as_ref().map(|i| i.server)?;
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
