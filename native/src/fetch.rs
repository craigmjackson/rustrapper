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

/// An open file handle in the pseudo-filesystem.
struct OpenFile {
    name: String,
    data: Vec<u8>,
    pos: usize,
    closed: bool,
    writable: bool,
}

static OPEN_FILES: Mutex<Vec<OpenFile>> = Mutex::new(Vec::new());

/// Host `io.open()` callback.
pub fn io_open(name: &str, mode: &str) -> Option<u8> {
    let mut files = OPEN_FILES.lock().unwrap();
    let writable = mode.contains('w') || mode.contains('a') || mode.contains('+');
    let data = if mode.contains('r') && !mode.contains('+') {
        // read-only: find in fetched files
        let fl = FILES.lock().unwrap();
        fl.iter().find(|(n, _)| n == name).map(|(_, d)| d.clone())?
    } else {
        Vec::new()
    };
    // append mode
    let data = if mode.contains('a') {
        let fl = FILES.lock().unwrap();
        fl.iter().find(|(n, _)| n == name).map(|(_, d)| d.clone()).unwrap_or(data)
    } else {
        data
    };
    let id = files.len() as u8;
    if id == u8::MAX { return None; }
    files.push(OpenFile {
        name: name.to_string(),
        data,
        pos: 0,
        closed: false,
        writable,
    });
    Some(id)
}

/// Host `io.close()` callback.
pub fn io_close(id: u8) -> bool {
    let mut files = OPEN_FILES.lock().unwrap();
    if let Some(f) = files.get_mut(id as usize) {
        f.closed = true;
        true
    } else {
        false
    }
}

/// Host `io.read()` callback. fmt: 0=all, 1=line, 2=N bytes.
pub fn io_read(id: u8, fmt: u8, n: usize, buf: &mut [u8]) -> Option<usize> {
    let mut files = OPEN_FILES.lock().unwrap();
    let f = files.get_mut(id as usize)?;
    if f.closed { return None; }
    match fmt {
        0 => {
            // read all remaining
            let remaining = &f.data[f.pos..];
            let nr = remaining.len().min(buf.len());
            if nr == 0 { return None; } // EOF
            buf[..nr].copy_from_slice(&remaining[..nr]);
            f.pos += nr;
            Some(nr)
        }
        1 => {
            // read one line (up to \n, not including \n)
            if f.pos >= f.data.len() { return None; } // EOF
            let start = f.pos;
            let mut end = start;
            while end < f.data.len() && f.data[end] != b'\n' {
                end += 1;
            }
            let nr = (end - start).min(buf.len());
            buf[..nr].copy_from_slice(&f.data[start..start + nr]);
            f.pos = if end < f.data.len() { end + 1 } else { end };
            if nr == 0 && start == f.data.len() { return None; }
            Some(nr)
        }
        _ => {
            // read up to n bytes
            let n = if n == 0 { buf.len() } else { n };
            let remaining = &f.data[f.pos..];
            let nr = remaining.len().min(n).min(buf.len());
            if nr == 0 { return None; }
            buf[..nr].copy_from_slice(&remaining[..nr]);
            f.pos += nr;
            Some(nr)
        }
    }
}

/// Host `io.write()` callback.
pub fn io_write(id: u8, data: &[u8]) -> bool {
    let mut files = OPEN_FILES.lock().unwrap();
    if let Some(f) = files.get_mut(id as usize) {
        if f.closed || !f.writable { return false; }
        f.data.extend_from_slice(data);
        true
    } else {
        false
    }
}

/// Host `io.flush()` callback.
pub fn io_flush(id: u8) -> bool {
    let files = OPEN_FILES.lock().unwrap();
    files.get(id as usize).map(|f| !f.closed).unwrap_or(false)
}

/// Host `io.type()` callback.
pub fn io_type(id: u8) -> Option<&'static str> {
    let files = OPEN_FILES.lock().unwrap();
    files.get(id as usize).map(|f| if f.closed { "closed file" } else { "file" })
}

/// Host `io.tmpfile()` is handled by io_open("", "w+").

/// Host `io.lines` advance: read next line from handle id into buf.
/// (io.lines iterator calls io_read with fmt=1.)

/// Wire up io callbacks (called from cli/main after creating state).
pub fn set_io_callbacks(state: &mut lua::LuaState) {
    state.set_io_open(Some(io_open));
    state.set_io_close(Some(io_close));
    state.set_io_read(Some(io_read));
    state.set_io_write(Some(io_write));
    state.set_io_flush(Some(io_flush));
    state.set_io_type(Some(io_type));
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
