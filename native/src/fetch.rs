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

/// Host `fetch()` callback: download `name` from the TFTP/HTTP server, keep it
/// in memory under the local name `save_as`, and return its byte count (or
/// `None` on failure). `name` may be prefixed with `tftp://` or `http://`;
/// a missing prefix defaults to `tftp://`.
pub fn fetch_file(name: &str, save_as: &str) -> Option<usize> {
    let data = if name.starts_with("https://") {
        #[cfg(feature = "https")]
        {
            https_download(&name["https://".len()..])?
        }
        #[cfg(not(feature = "https"))]
        {
            return None;
        }
    } else if let Some(rest) = name.strip_prefix("http://") {
        http_download(rest)?
    } else {
        let name = name.strip_prefix("tftp://").unwrap_or(name);
        let server = CTX.lock().unwrap().as_ref().map(|i| i.server)?;
        if server == Ipv4Addr::UNSPECIFIED {
            return None;
        }
        let mut data = Vec::new();
        {
            let mut sink = FileSink { data: &mut data };
            crate::net::tftp_download(server, name, &mut sink)?;
        }
        data
    };
    FILES.lock().unwrap().push((save_as.to_string(), data.clone()));
    Some(data.len())
}

#[cfg(feature = "https")]
fn https_download(url: &str) -> Option<Vec<u8>> {
    use rustls::pki_types::ServerName;
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;
    use std::sync::Arc;

    let (hostport, path) = match url.find('/') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, "/"),
    };
    let (host, port) = match hostport.find(':') {
        Some(i) => (&hostport[..i], hostport[i + 1..].parse::<u16>().ok()?),
        None => (hostport, 443u16),
    };

    // Install ring as the crypto provider (idempotent).
    let _ = rustls::crypto::ring::default_provider().install_default();

    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let server_name = ServerName::try_from(host.to_string()).ok()?;
    let conn = rustls::ClientConnection::new(Arc::new(config), server_name).ok()?;

    let addr = format!("{}:{}", host, port);
    let tcp = std::net::TcpStream::connect_timeout(
        &addr.to_socket_addrs().ok()?.next()?,
        std::time::Duration::from_secs(10),
    )
    .ok()?;
    let mut tls = rustls::StreamOwned::new(conn, tcp);

    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, host
    );
    tls.write_all(req.as_bytes()).ok()?;
    let mut resp = Vec::new();
    tls.read_to_end(&mut resp).ok()?;

    let sep = b"\r\n\r\n";
    let idx = resp.windows(4).position(|w| w == sep)?;
    Some(resp[idx + 4..].to_vec())
}

/// Minimal HTTP/1.0 GET client. `url` is `host[:port]/path` (no scheme).
fn http_download(url: &str) -> Option<Vec<u8>> {
    // Split host[:port] and path.
    let (hostport, path) = match url.find('/') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, "/"),
    };
    let (host, port) = match hostport.find(':') {
        Some(i) => (&hostport[..i], hostport[i + 1..].parse::<u16>().ok()?),
        None => (hostport, 80u16),
    };
    // Resolve host (IP literal only for simplicity; fall back to DNS not done).
    use std::net::ToSocketAddrs;
    let addrs = format!("{}:{}", host, port).to_socket_addrs().ok()?;
    let mut stream = None;
    for a in addrs {
        if let Ok(s) = std::net::TcpStream::connect_timeout(&a, std::time::Duration::from_secs(10)) {
            stream = Some(s);
            break;
        }
    }
    let mut stream = stream?;
    use std::io::{Read, Write};
    let req = format!(
        "GET {} HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
        path, host
    );
    stream.write_all(req.as_bytes()).ok()?;
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).ok()?;
    // Split headers and body.
    let sep = b"\r\n\r\n";
    let idx = resp.windows(4).position(|w| w == sep)?;
    let body = &resp[idx + 4..];
    Some(body.to_vec())
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
        let fl = FILES.lock().unwrap();
        fl.iter().find(|(n, _)| n == name).map(|(_, d)| d.clone())?
    } else if mode.contains('a') {
        let fl = FILES.lock().unwrap();
        fl.iter().find(|(n, _)| n == name).map(|(_, d)| d.clone()).unwrap_or_default()
    } else {
        Vec::new()
    };
    // For write/append modes, register a pseudo-fs entry so ls can see it.
    if writable && !name.is_empty() {
        let mut fl = FILES.lock().unwrap();
        if !fl.iter().any(|(n, _)| n == name) {
            fl.push((name.to_string(), data.clone()));
        }
    }
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
        if f.writable && !f.name.is_empty() {
            let data = f.data.clone();
            let name = f.name.clone();
            drop(files);
            let mut fl = FILES.lock().unwrap();
            if let Some(entry) = fl.iter_mut().find(|(n, _)| *n == name) {
                entry.1 = data;
            } else {
                fl.push((name, data));
            }
            let mut files = OPEN_FILES.lock().unwrap();
            if let Some(f) = files.get_mut(id as usize) {
                f.closed = true;
            }
            true
        } else {
            f.closed = true;
            true
        }
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

pub fn io_size(id: u8) -> usize {
    let files = OPEN_FILES.lock().unwrap();
    if let Some(f) = files.get(id as usize) {
        if f.closed {
            // look up in FILES
            let fl = FILES.lock().unwrap();
            fl.iter().find(|(n, _)| *n == f.name).map(|(_, d)| d.len()).unwrap_or(f.data.len())
        } else {
            f.data.len()
        }
    } else {
        0
    }
}

/// Host `io.type()` callback.
pub fn io_type(id: u8) -> Option<&'static str> {
    let files = OPEN_FILES.lock().unwrap();
    files.get(id as usize).map(|f| if f.closed { "closed file" } else { "file" })
}

pub fn io_seek(id: u8, whence: &str, offset: i64) -> Option<usize> {
    let mut files = OPEN_FILES.lock().unwrap();
    let f = files.get_mut(id as usize)?;
    if f.closed { return None; }
    let new_pos = match whence {
        "set" => offset,
        "cur" => f.pos as i64 + offset,
        "end" => f.data.len() as i64 + offset,
        _ => return None,
    };
    if new_pos < 0 || new_pos as usize > f.data.len() { return None; }
    f.pos = new_pos as usize;
    Some(f.pos + 1) // Lua returns 1-based position
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
    let data = if name.starts_with("https://") {
        #[cfg(feature = "https")]
        {
            https_download(&name["https://".len()..])?
        }
        #[cfg(not(feature = "https"))]
        {
            return None;
        }
    } else if let Some(rest) = name.strip_prefix("http://") {
        http_download(rest)?
    } else {
        let name = name.strip_prefix("tftp://").unwrap_or(name);
        let server = CTX.lock().unwrap().as_ref().map(|i| i.server)?;
        if server == Ipv4Addr::UNSPECIFIED {
            return None;
        }
        let mut data = Vec::new();
        {
            let mut sink = FileSink { data: &mut data };
            crate::net::tftp_download(server, name, &mut sink)?;
        }
        data
    };
    if data.len() > buf.len() {
        return None;
    }
    buf[..data.len()].copy_from_slice(&data);
    Some(data.len())
}

pub fn os_remove(name: &str) -> bool {
    let mut fl = FILES.lock().unwrap();
    if let Some(i) = fl.iter().position(|(n, _)| n == name) {
        fl.remove(i);
        true
    } else {
        false
    }
}

pub fn os_rename(old: &str, new: &str) -> bool {
    let mut fl = FILES.lock().unwrap();
    if let Some(i) = fl.iter().position(|(n, _)| n == old) {
        let data = fl[i].1.clone();
        fl.remove(i);
        if let Some(j) = fl.iter().position(|(n, _)| n == new) {
            fl[j].1 = data;
        } else {
            fl.push((new.to_string(), data));
        }
        true
    } else {
        false
    }
}
