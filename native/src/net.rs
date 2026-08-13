//! Kernel-backed networking for the native target.
//!
//! The running OS handles routing and sockets; this module only discovers the
//! local IP / TFTP server from the kernel (`/proc/net/route` + the UDP
//! connect-to-discover-local-IP trick) and runs a TFTP client over std UDP
//! sockets. The wire protocol helpers (`build_rrq`, `parse_data`, `build_ack`,
//! `parse_oack`, `TftpSink`) come from `common::tftp`.

use std::net::{Ipv4Addr, UdpSocket};
use std::time::Duration;

use common::print::{print_dec, print_ip, putc, puts};
use common::tftp::{build_ack, build_rrq, parse_data, parse_oack, DEFAULT_BLKSIZE, TftpSink};

/// TFTP server override, e.g. `RUSTWRAPPER_TFTP_SERVER=10.0.0.1`.
const ENV_SERVER: &str = "RUSTWRAPPER_TFTP_SERVER";
/// TFTP server port override (default 69), e.g. `RUSTWRAPPER_TFTP_PORT=1069`.
const ENV_PORT: &str = "RUSTWRAPPER_TFTP_PORT";
/// PXE bootfile name for menu option 2.
const ENV_BOOTFILE: &str = "RUSTWRAPPER_BOOTFILE";

/// Discover the (TFTP server, local IP) from the kernel and record it for the
/// `fetch()` callback. The server defaults to the default gateway (the PXE
/// server in the QEMU test setups), overridable via `RUSTWRAPPER_TFTP_SERVER`.
pub fn setup_fetch_context() -> bool {
    let (server, local) = match discover_network() {
        Some(x) => x,
        None => {
            puts("  no network route found (set RUSTWRAPPER_TFTP_SERVER)\r\n");
            return false;
        }
    };
    puts("  local IP: ");
    print_ip(&local.octets());
    putc(b'\r');
    putc(b'\n');
    puts("  TFTP server: ");
    print_ip(&server.octets());
    putc(b'\r');
    putc(b'\n');
    crate::fetch::set_context(server, local);
    true
}

/// Host `dhcp` builtin: set up the network and return the `fetch` callback if
/// a TFTP server is reachable.
pub fn dhcp_fn() -> Option<fn(&str) -> Option<usize>> {
    if setup_fetch_context() {
        Some(crate::fetch::fetch_file)
    } else {
        None
    }
}

/// Menu option 2: download the PXE bootfile over TFTP and run/display it.
pub fn network_boot() {
    if !setup_fetch_context() {
        return;
    }
    let server = crate::fetch::context().0;
    let bootfile =
        std::env::var(ENV_BOOTFILE).unwrap_or_else(|_| "test.lua".to_string());
    puts("PXE: downloading ");
    puts(&bootfile);
    putc(b'\r');
    putc(b'\n');

    let mut data = Vec::new();
    let size = {
        let mut sink = VecSink { data: &mut data };
        tftp_download(server, &bootfile, &mut sink)
    };
    match size {
        Some(size) => {
            puts("PXE: downloaded ");
            print_dec(size as u64);
            puts(" bytes\r\n");
            if bootfile.ends_with(".lua") {
                puts("PXE: executing Lua script\r\n");
                match lua::run_with_fetch_load(
                    &data,
                    putc,
                    crate::fetch::fetch_file,
                    crate::fetch::load_file,
                ) {
                    Ok(()) => puts("PXE: Lua script done\r\n"),
                    Err(e) => {
                        puts("Lua error: ");
                        puts(e);
                        putc(b'\r');
                        putc(b'\n');
                    }
                }
            } else if let Ok(text) = std::str::from_utf8(&data) {
                puts("--- file contents ---\r\n");
                puts(text);
                if !text.ends_with('\n') {
                    putc(b'\r');
                    putc(b'\n');
                }
            }
        }
        None => puts("PXE: download failed\r\n"),
    }
}

fn discover_network() -> Option<(Ipv4Addr, Ipv4Addr)> {
    let server = match std::env::var(ENV_SERVER) {
        Ok(v) => v.parse::<Ipv4Addr>().ok()?,
        Err(_) => default_gateway()?,
    };
    let local = local_ip_to(server)?;
    Some((server, local))
}

/// Read the default gateway from `/proc/net/route` (little-endian hex fields).
fn default_gateway() -> Option<Ipv4Addr> {
    let data = std::fs::read_to_string("/proc/net/route").ok()?;
    for line in data.lines().skip(1) {
        let mut f = line.split_whitespace();
        let _iface = f.next()?;
        let dest = f.next()?;
        let gw = f.next()?;
        if dest == "00000000" && gw != "00000000" {
            return parse_route_hex(gw);
        }
    }
    None
}

/// Decode a `/proc/net/route` hex field (little-endian: LSB of the u32 is the
/// first octet of the IP).
fn parse_route_hex(hex: &str) -> Option<Ipv4Addr> {
    let g = u32::from_str_radix(hex, 16).ok()?;
    Some(Ipv4Addr::new(
        (g & 0xff) as u8,
        ((g >> 8) & 0xff) as u8,
        ((g >> 16) & 0xff) as u8,
        ((g >> 24) & 0xff) as u8,
    ))
}

/// Discover the local IP the kernel would use to reach `dst` by connecting a
/// bound UDP socket (no packets are sent).
fn local_ip_to(dst: Ipv4Addr) -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect((dst, 9)).ok()?;
    match sock.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) => Some(ip),
        _ => None,
    }
}

/// A [`TftpSink`] that appends blocks to a `Vec<u8>`.
pub struct VecSink<'a> {
    pub data: &'a mut Vec<u8>,
}

impl TftpSink for VecSink<'_> {
    fn write_block(&mut self, data: &[u8]) -> Result<(), ()> {
        self.data.extend_from_slice(data);
        Ok(())
    }
    fn finalize(&mut self, _size: usize) -> Result<(), ()> {
        Ok(())
    }
}

/// Download `filename` from `server` over TFTP (RFC 1350) using a std UDP
/// socket, streaming blocks into `sink`. Returns the total size on success.
pub fn tftp_download(server: Ipv4Addr, filename: &str, sink: &mut impl TftpSink) -> Option<usize> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let port = std::env::var(ENV_PORT)
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(69);

    let mut rrq = [0u8; 256];
    let rrq_len = build_rrq(filename, &mut rrq);
    sock.send_to(&rrq[..rrq_len], (server, port)).ok()?;

    let mut blksize = DEFAULT_BLKSIZE;
    let mut total = 0usize;
    let mut buf = [0u8; 2048];
    loop {
        let (n, src) = sock.recv_from(&mut buf).ok()?;
        if src.ip() != std::net::IpAddr::V4(server) {
            continue;
        }
        let len = n.min(buf.len());
        if let Some((bs, _)) = parse_oack(&buf, len) {
            blksize = bs;
            let mut ack = [0u8; 4];
            let alen = build_ack(0, &mut ack);
            sock.send_to(&ack[..alen], (server, src.port())).ok()?;
            continue;
        }
        if let Some((block, data)) = parse_data(&buf, len) {
            if sink.write_block(data).is_err() {
                return None;
            }
            total += data.len();
            let mut ack = [0u8; 4];
            let alen = build_ack(block, &mut ack);
            sock.send_to(&ack[..alen], (server, src.port())).ok()?;
            if data.len() < blksize {
                break;
            }
        }
    }
    sink.finalize(total).ok()?;
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_hex_decodes_little_endian() {
        assert_eq!(parse_route_hex("0100000A"), Some(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(parse_route_hex("0202000A"), Some(Ipv4Addr::new(10, 0, 2, 2)));
        assert_eq!(parse_route_hex("0A000001"), Some(Ipv4Addr::new(1, 0, 0, 10)));
        assert_eq!(parse_route_hex("00000000"), Some(Ipv4Addr::new(0, 0, 0, 0)));
        assert_eq!(parse_route_hex("zz"), None);
    }

    #[test]
    fn rrq_builds_via_common() {
        // The native TFTP client reuses common::tftp's wire protocol helpers.
        let mut buf = [0u8; 256];
        let len = common::tftp::build_rrq("test.txt", &mut buf);
        assert_eq!(&buf[..len], b"\x00\x01test.txt\x00octet\x00");
    }
}
