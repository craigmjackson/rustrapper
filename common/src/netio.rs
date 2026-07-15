//! e1000-based network I/O glue: ARP resolve and DNS lookup.
//!
//! These functions use `common::e1000` for send/receive and `common::print`
//! for output. They are shared by `bios` and `arm64-bare` (both use the
//! global print callback). The UEFI target implements its own glue because it
//! uses `con_out` for output and may use SNP instead of direct e1000.

#[cfg(not(test))]
use crate::arp;
#[cfg(not(test))]
use crate::dns;
#[cfg(not(test))]
use crate::e1000;
#[cfg(not(test))]
use crate::print::{print_ip, puts, putc};

/// Check if two IPs are on the same subnet given a netmask.
pub fn same_subnet(ip1: [u8; 4], ip2: [u8; 4], mask: [u8; 4]) -> bool {
    for i in 0..4 {
        if ip1[i] & mask[i] != ip2[i] & mask[i] {
            return false;
        }
    }
    true
}

/// Resolve `dst_ip` to a MAC address via ARP over e1000.
/// Sends a broadcast ARP request and polls for a reply.
#[cfg(not(test))]
pub fn arp_resolve_e1000(
    base: u64,
    src_mac: &[u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
) -> Option<[u8; 6]> {
    let mut frame = [0u8; 1514];
    let len = arp::build_request(src_mac, src_ip, dst_ip, &mut frame);
    if !e1000::send(base, &frame[..len]) {
        return None;
    }

    let mut buf = [0u8; 1514];
    for _ in 0..20 {
        if let Some(len) = e1000::try_receive(base, &mut buf, 50_000_000) {
            if let Some(mac) = arp::parse_reply(&buf, len, dst_ip) {
                return Some(mac);
            }
        }
    }
    None
}

/// Send a DNS query for `name` to `dst_ip` (via `dst_mac`) over e1000.
/// Returns the first A-record IP address from the response.
#[cfg(not(test))]
pub fn dns_lookup_e1000(
    base: u64,
    src_mac: &[u8; 6],
    src_ip: [u8; 4],
    dst_mac: &[u8; 6],
    dst_ip: [u8; 4],
    name: &str,
) -> Option<[u8; 4]> {
    let id: u16 = 0x1234;
    let src_port: u16 = 0x3039;

    let mut query = [0u8; 256];
    let query_len = dns::build_query(id, name, &mut query);
    if query_len == 0 {
        return None;
    }

    let mut frame = [0u8; 1514];
    let frame_len = dns::build_frame(
        src_mac,
        src_ip,
        dst_mac,
        dst_ip,
        src_port,
        &query,
        query_len,
        &mut frame,
    );
    if !e1000::send(base, &frame[..frame_len]) {
        return None;
    }

    let mut buf = [0u8; 1514];
    for _ in 0..20 {
        if let Some(len) = e1000::try_receive(base, &mut buf, 100_000_000) {
            if let Some(ip) = dns::parse_response(&buf, len, id) {
                return Some(ip);
            }
        }
    }
    None
}

/// Full DNS resolve flow: print DNS server, ARP resolve next-hop, query
/// `google.com`, and print the result. Uses the global print callback.
#[cfg(not(test))]
pub fn dns_resolve_and_print(base: u64, mac: &[u8; 6], cfg: &crate::dhcp::DhcpConfig) {
    if cfg.dns == [0u8; 4] {
        puts("    DNS: (none)\n");
        return;
    }

    puts("    DNS server: ");
    print_ip(&cfg.dns);
    putc(b'\n');

    // Determine next-hop: DNS server if on-subnet, gateway otherwise
    let next_hop = if same_subnet(cfg.yiaddr, cfg.dns, cfg.subnet) {
        cfg.dns
    } else if cfg.gateway != [0u8; 4] {
        cfg.gateway
    } else {
        puts("    Cannot resolve: no gateway for off-subnet DNS\n");
        return;
    };

    puts("    ARP ");
    print_ip(&next_hop);
    puts("...");
    let dst_mac = match arp_resolve_e1000(base, mac, cfg.yiaddr, next_hop) {
        Some(m) => m,
        None => {
            puts("FAILED\n");
            return;
        }
    };
    puts("OK\n");

    puts("    DNS: google.com...");
    match dns_lookup_e1000(base, mac, cfg.yiaddr, &dst_mac, cfg.dns, "google.com") {
        Some(ip) => {
            puts("OK\n");
            puts("    google.com: ");
            print_ip(&ip);
            putc(b'\n');
        }
        None => {
            puts("FAILED\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_same_subnet_same() {
        assert!(same_subnet([10, 0, 2, 15], [10, 0, 2, 2], [255, 255, 255, 0]));
    }

    #[test]
    fn test_same_subnet_different() {
        assert!(!same_subnet([10, 0, 2, 15], [10, 0, 3, 2], [255, 255, 255, 0]));
    }

    #[test]
    fn test_same_subnet_class_b() {
        assert!(same_subnet([172, 16, 1, 5], [172, 16, 2, 10], [255, 255, 0, 0]));
        assert!(!same_subnet([172, 16, 1, 5], [172, 17, 2, 10], [255, 255, 0, 0]));
    }

    #[test]
    fn test_same_subnet_full_mask() {
        assert!(same_subnet([10, 0, 0, 1], [10, 0, 0, 1], [255, 255, 255, 255]));
        assert!(!same_subnet([10, 0, 0, 1], [10, 0, 0, 2], [255, 255, 255, 255]));
    }

    #[test]
    fn test_same_subnet_zero_mask() {
        assert!(same_subnet([10, 0, 0, 1], [192, 168, 1, 1], [0, 0, 0, 0]));
    }
}
