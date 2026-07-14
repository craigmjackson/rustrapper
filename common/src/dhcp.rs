//! DHCP DISCOVER build, Ethernet/IP/UDP/DHCP frame build, and response parse.
//!
//! Validates:
//! - DISCOVER has correct op/htype/hlen/magic cookie/option 53/option 55/option 255
//! - Padding fills unused bytes to 300
//! - `build_eth_ip_udp` produces a well-formed Ethernet+IPv4+UDP+DHCP frame
//! - IP header checksum is correct
//! - `parse_response` accepts both OFFER (type 2) and ACK (type 5)
//! - `parse_response` extracts yiaddr, subnet mask (option 1), router (option 3)
//! - `parse_response` rejects mismatched xid or MAC
//! - `parse_response` rejects non-UDP, bad magic cookie, too-short packets
//! - `parse_response` rejects non-OFFER/ACK messages

/// A successful DHCP result: the IP address we were assigned, plus subnet, gateway, and nameserver.
#[derive(Clone, Copy)]
pub struct DhcpConfig {
    pub yiaddr: [u8; 4],
    pub subnet: [u8; 4],
    pub gateway: [u8; 4],
    pub nameserver: [u8; 4],
}

/// Compute the standard Internet checksum (one's complement of the one's
/// complement sum of 16-bit words).
pub fn ip_checksum(buf: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < buf.len() {
        sum += (buf[i] as u32) << 8 | buf[i + 1] as u32;
        i += 2;
    }
    if i < buf.len() {
        sum += (buf[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Build a DHCPDISCOVER payload (300 bytes).
/// Options: magic cookie 0x63825363, DHCP type 1 (DISCOVER), parameter
/// request list for subnet (1), router (3), DNS (6), end (255).
pub fn build_discover(xid: u32, mac: &[u8; 6]) -> [u8; 300] {
    let mut pkt = [0u8; 300];
    pkt[0] = 1; // op = BOOTREQUEST
    pkt[1] = 1; // htype = Ethernet
    pkt[2] = 6; // hlen = 6 (MAC)
    pkt[4..8].copy_from_slice(&xid.to_be_bytes());
    pkt[10] = 0x80; // broadcast flag
    pkt[28..34].copy_from_slice(mac);
    pkt[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);

    let mut off = 240;
    pkt[off] = 53;
    pkt[off + 1] = 1;
    pkt[off + 2] = 1; // DISCOVER
    off += 3;
    pkt[off] = 55;
    pkt[off + 1] = 3;
    pkt[off + 2] = 1; // subnet
    pkt[off + 3] = 3; // router
    pkt[off + 4] = 6; // DNS
    off += 5;
    pkt[off] = 255; // end

    pkt
}

/// Build the full Ethernet + IPv4 + UDP + DHCP frame into `frame`.
/// Returns the total frame length written.
pub fn build_eth_ip_udp(
    mac: &[u8; 6],
    dhcp_payload: &[u8],
    dhcp_len: usize,
    frame: &mut [u8; 1514],
) -> usize {
    // Ethernet header
    frame[0..6].fill(0xFF); // broadcast destination
    frame[6..12].copy_from_slice(mac);
    frame[12] = 0x08;
    frame[13] = 0x00; // EtherType IPv4

    // IPv4 header
    let ip_off = 14;
    let ip_total_len = 20 + 8 + dhcp_len;
    frame[ip_off] = 0x45;
    frame[ip_off + 1] = 0x00;
    frame[ip_off + 2..ip_off + 4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    frame[ip_off + 8] = 64; // TTL
    frame[ip_off + 9] = 17; // protocol = UDP
    frame[ip_off + 12..ip_off + 16].fill(0x00); // src IP
    frame[ip_off + 16..ip_off + 20].fill(0xFF); // dst IP (broadcast)

    let cksum = ip_checksum(&frame[ip_off..ip_off + 20]);
    frame[ip_off + 10..ip_off + 12].copy_from_slice(&cksum.to_be_bytes());

    // UDP header
    let udp_off = ip_off + 20;
    let udp_len = 8 + dhcp_len;
    frame[udp_off..udp_off + 2].copy_from_slice(&[0x00, 0x44]); // src port 68
    frame[udp_off + 2..udp_off + 4].copy_from_slice(&[0x00, 0x43]); // dst port 67
    frame[udp_off + 4..udp_off + 6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    // UDP checksum = 0 (optional for UDP over IPv4)

    // DHCP payload
    let dhcp_off = udp_off + 8;
    frame[dhcp_off..dhcp_off + dhcp_len].copy_from_slice(&dhcp_payload[..dhcp_len]);
    dhcp_off + dhcp_len
}

/// Parse a DHCP response. Validates the magic cookie, the transaction ID, and
/// the MAC address, then walks the options to extract yiaddr, subnet, and gateway.
pub fn parse_response(buf: &[u8], len: usize, xid: u32, mac: &[u8; 6]) -> Option<DhcpConfig> {
    if len < 282 {
        return None;
    }
    if buf[12] != 0x08 || buf[13] != 0x00 {
        return None;
    }

    let ip_off = 14;
    let ip_hdr_len = (buf[ip_off] & 0x0F) as usize * 4;
    if ip_hdr_len < 20 {
        return None;
    }
    if buf[ip_off + 9] != 17 {
        return None;
    } // not UDP

    let udp_off = ip_off + ip_hdr_len;
    let dhcp_off = udp_off + 8;
    let _dhcp_len = len - dhcp_off;
    if _dhcp_len < 240 {
        return None;
    }
    if dhcp_off + 4 > len {
        return None;
    }
    // Magic cookie
    if buf[dhcp_off + 236] != 0x63
        || buf[dhcp_off + 237] != 0x82
        || buf[dhcp_off + 238] != 0x53
        || buf[dhcp_off + 239] != 0x63
    {
        return None;
    }

    // Transaction ID
    let pkt_xid = u32::from_be_bytes([
        buf[dhcp_off + 4],
        buf[dhcp_off + 5],
        buf[dhcp_off + 6],
        buf[dhcp_off + 7],
    ]);
    if pkt_xid != xid {
        return None;
    }

    // MAC match
    for i in 0..6 {
        if buf[dhcp_off + 28 + i] != mac[i] {
            return None;
        }
    }

    // yiaddr
    let yiaddr: [u8; 4] = [
        buf[dhcp_off + 16],
        buf[dhcp_off + 17],
        buf[dhcp_off + 18],
        buf[dhcp_off + 19],
    ];

    let mut subnet = [255u8; 4];
    let mut gateway = [0u8; 4];
    let mut nameserver = [0u8; 4];
    let mut dhcp_msg_type = 0u8;
    let mut off = dhcp_off + 240;

    while off + 1 < len {
        let opt_type = buf[off];
        if opt_type == 255 {
            break;
        }
        let opt_len = buf[off + 1] as usize;
        if off + 2 + opt_len > len {
            break;
        }

        if opt_type == 53 && opt_len == 1 {
            dhcp_msg_type = buf[off + 2];
        } else if opt_type == 1 && opt_len == 4 {
            subnet.copy_from_slice(&buf[off + 2..off + 6]);
        } else if opt_type == 3 && opt_len >= 4 {
            gateway.copy_from_slice(&buf[off + 2..off + 6]);
        } else if opt_type == 6 && opt_len >= 4 && nameserver == [0u8; 4] {
            nameserver.copy_from_slice(&buf[off + 2..off + 6]);
        }
        off += 2 + opt_len;
    }

    // Accept both OFFER (2) and ACK (5) — single-transmit sends DISCOVER
    // and accepts the OFFER as final.
    if dhcp_msg_type != 2 && dhcp_msg_type != 5 {
        return None;
    }

    Some(DhcpConfig {
        yiaddr,
        subnet,
        gateway,
        nameserver,
    })
}

/// Encode a domain name into DNS wire format (label list terminated by 0x00).
/// Returns Some((encoded_bytes, actual_length)) or None if the name is too long.
fn encode_domain(name: &str) -> Option<([u8; 256], usize)> {
    let mut buf = [0u8; 256];
    let mut off = 0;
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        buf[off] = label.len() as u8;
        off += 1;
        for byte in label.as_bytes() {
            buf[off] = *byte;
            off += 1;
        }
    }
    buf[off] = 0; // root label terminator
    off += 1;
    Some((buf, off))
}

/// Parse a DNS response and return the first A record (IPv4) address.
/// Expects `buf` to start with an Ethernet header (skips Ethernet + IPv4 + UDP).
/// Returns None if the response is invalid or contains no A records.
pub fn parse_dns_response(buf: &[u8], len: usize, txid: u16) -> Option<[u8; 4]> {
    if len < 42 {
        return None;
    }
    // Skip Ethernet header (14 bytes)
    if buf[12] != 0x08 || buf[13] != 0x00 {
        return None;
    }
    let ip_off = 14;
    let ip_hdr_len = (buf[ip_off] & 0x0F) as usize * 4;
    if ip_hdr_len < 20 {
        return None;
    }
    if buf[ip_off + 9] != 17 {
        return None;
    } // not UDP
    let udp_off = ip_off + ip_hdr_len;
    let dns_off = udp_off + 8;
    if dns_off + 12 > len {
        return None;
    }
    let pkt_txid = u16::from_be_bytes([buf[dns_off], buf[dns_off + 1]]);
    if pkt_txid != txid {
        return None;
    }
    let rcode = buf[dns_off + 2] & 0x0F;
    if rcode != 0 {
        return None;
    }
    let ancount = u16::from_be_bytes([buf[dns_off + 6], buf[dns_off + 7]]) as usize;
    if ancount == 0 {
        return None;
    }
    // Skip past the question section (DNS header is 12 bytes)
    let mut off = dns_off + 12;
    // Skip name (walk until 0x00 label)
    while off < len {
        let label_len = buf[off];
        if label_len == 0 {
            off += 1;
            break;
        }
        if label_len & 0xC0 == 0xC0 {
            // Compression pointer — skip 2 bytes
            off += 2;
            break;
        }
        if label_len & 0xC0 != 0 {
            return None; // invalid label type
        }
        off += 1 + label_len as usize;
    }
    // Question: type (2) + class (2)
    if off + 4 > len {
        return None;
    }
    off += 4;

    // Walk answer records
    for _ in 0..ancount {
        if off >= len {
            return None;
        }
        // Skip name (may use compression)
        if buf[off] & 0xC0 == 0xC0 {
            off += 2;
        } else {
            while off < len {
                let ll = buf[off];
                if ll == 0 {
                    off += 1;
                    break;
                }
                off += 1 + ll as usize;
            }
        }
        // type (2) + class (2) + ttl (4) + rdlength (2)
        if off + 10 > len {
            return None;
        }
        let rtype = u16::from_be_bytes([buf[off], buf[off + 1]]);
        off += 10;
        let rdlength = u16::from_be_bytes([buf[off - 2], buf[off - 1]]) as usize;
        if rtype != 1 {
            // Not an A record, skip
            off += rdlength;
            continue;
        }
        if off + rdlength > len {
            return None;
        }
        let mut addr = [0u8; 4];
        addr.copy_from_slice(&buf[off..off + rdlength]);
        return Some(addr);
    }
    None
}

/// Build a DNS query for an A record of the given domain name.
/// Returns the encoded packet or None if the name is too long.
pub fn build_dns_query(name: &str, txid: u16) -> Option<[u8; 256]> {
    let (encoded, elen) = encode_domain(name)?;
    let mut pkt = [0u8; 256];

    // Header
    pkt[0..2].copy_from_slice(&txid.to_be_bytes());
    pkt[2] = 0x01; pkt[3] = 0x00; // flags: standard query, recursion desired
    pkt[4] = 0x00; pkt[5] = 0x01; // qdcount = 1
    // ancount, nscount, arcount = 0 (already zero)

    // Question: encoded domain + type A (1) + class IN (1)
    let qoff = 12;
    pkt[qoff..qoff + elen].copy_from_slice(&encoded[..elen]);
    let qend = qoff + elen;
    pkt[qend] = 0x00; pkt[qend + 1] = 0x01; // type = A
    pkt[qend + 2] = 0x00; pkt[qend + 3] = 0x01; // class = IN

    Some(pkt)
}

/// Send a DNS query via raw e1000 MMIO and return the resolved IPv4 address.
/// Uses the provided NIC base address, MAC, our IP, ARP target, and nameserver IP.
/// Returns None on timeout or parse failure.
#[cfg(not(test))]
pub fn dns_lookup_via_e1000(
    nic_base: u64,
    mac: &[u8; 6],
    our_ip: &[u8; 4],
    arp_target: &[u8; 4],
    nameserver: &[u8; 4],
    domain: &str,
) -> Option<[u8; 4]> {
    let txid: u16 = 0x1234;
    let query = build_dns_query(domain, txid)?;
    let qlen = query.len();

    // Find first non-zero byte to determine actual query length
    let mut actual_len = qlen;
    for i in (0..qlen).rev() {
        if query[i] != 0 {
            actual_len = i + 1;
            break;
        }
    }

    // Try ARP first, fall back to broadcast MAC for QEMU slirp
    let dst_mac = match arp_resolve(nic_base, mac, our_ip, arp_target) {
        Some(m) => m,
        None => {
            if arp_target[0] == 10 && arp_target[1] == 0 && arp_target[2] == 2 {
                [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]
            } else {
                return None;
            }
        }
    };

    let mut frame = [0u8; 1514];
    let frame_len = build_dns_frame_with_mac(
        &dst_mac,
        mac,
        our_ip,
        nameserver,
        &query[..actual_len],
        actual_len,
        &mut frame,
    );

    if !super::e1000::send(nic_base, &frame[..frame_len]) {
        return None;
    }

    let mut buf = [0u8; 1514];
    for _ in 0..10_000_000 {
        if let Some(copy_len) = super::e1000::try_receive(nic_base, &mut buf, 1) {
            if let Some(ip) = parse_dns_response(&buf, copy_len, txid) {
                return Some(ip);
            }
        }
    }
    None
}

/// ARP request frame layout:
/// Ethernet dst: broadcast
/// Ethernet src: our MAC
/// EtherType: 0x0806 (ARP)
/// ARP: hw_type=1, proto=0x0800, hw_len=6, proto_len=4, op=1 (request)
/// Sender MAC + IP
/// Target MAC (zero) + IP
fn build_arp_request(mac: &[u8; 6], our_ip: &[u8; 4], target_ip: &[u8; 4], frame: &mut [u8; 1514]) -> usize {
    let mut pos = 0;
    // Ethernet header
    frame[pos..pos + 6].fill(0xFF); pos += 6;
    frame[pos..pos + 6].copy_from_slice(mac); pos += 6;
    frame[pos] = 0x08; frame[pos + 1] = 0x06; pos += 2; // EtherType ARP

    // ARP payload
    let arp_off = pos;
    frame[arp_off + 0..arp_off + 2].copy_from_slice(&[0x00, 0x01]); pos += 2; // hw type = Ethernet
    frame[arp_off + 2..arp_off + 4].copy_from_slice(&[0x08, 0x00]); pos += 2; // proto = IPv4
    frame[arp_off + 4] = 0x06; pos += 1; // hw len = 6
    frame[arp_off + 5] = 0x04; pos += 1; // proto len = 4
    frame[arp_off + 6..arp_off + 8].copy_from_slice(&[0x00, 0x01]); pos += 2; // op = request
    frame[arp_off + 8..arp_off + 14].copy_from_slice(mac); pos += 6; // sender MAC
    frame[arp_off + 14..arp_off + 18].copy_from_slice(our_ip); pos += 4; // sender IP
    frame[arp_off + 18..arp_off + 24].fill(0); pos += 6; // target MAC (zero)
    frame[arp_off + 24..arp_off + 28].copy_from_slice(target_ip); pos += 4; // target IP
    pos
}

/// Parse an ARP reply and extract the sender's MAC address and IP.
/// Returns None if not an ARP reply.
fn parse_arp_reply(buf: &[u8], len: usize) -> Option<([u8; 6], [u8; 4])> {
    if len < 42 {
        return None;
    }
    // Check EtherType = ARP
    if buf[12] != 0x08 || buf[13] != 0x06 {
        return None;
    }
    let arp_off = 14;
    // hw type = 1, proto = 0x0800
    if buf[arp_off] != 0x00 || buf[arp_off + 1] != 0x01 {
        return None;
    }
    if buf[arp_off + 2] != 0x08 || buf[arp_off + 3] != 0x00 {
        return None;
    }
    // op = 2 (reply)
    if buf[arp_off + 6] != 0x00 || buf[arp_off + 7] != 0x02 {
        return None;
    }
    // sender MAC is at offset 8 in ARP payload
    let sender_mac: [u8; 6] = [
        buf[arp_off + 8], buf[arp_off + 9], buf[arp_off + 10],
        buf[arp_off + 11], buf[arp_off + 12], buf[arp_off + 13],
    ];
    // sender IP is at offset 14 in ARP payload
    let sender_ip: [u8; 4] = [
        buf[arp_off + 14], buf[arp_off + 15],
        buf[arp_off + 16], buf[arp_off + 17],
    ];
    Some((sender_mac, sender_ip))
}

/// Resolve a target IP to a MAC address via ARP.
/// Sends an ARP request and polls for the reply.
/// Returns the resolved MAC or None on timeout.
#[cfg(not(test))]
fn arp_resolve(
    nic_base: u64,
    mac: &[u8; 6],
    our_ip: &[u8; 4],
    target_ip: &[u8; 4],
) -> Option<[u8; 6]> {
    let mut frame = [0u8; 1514];
    let frame_len = build_arp_request(mac, our_ip, target_ip, &mut frame);

    if !super::e1000::send(nic_base, &frame[..frame_len]) {
        return None;
    }

    let mut buf = [0u8; 1514];
    for _ in 0..1_000_000 {
        if let Some(len) = super::e1000::try_receive(nic_base, &mut buf, 100) {
            if len >= 42 && buf[12] == 0x08 && buf[13] == 0x06 {
                if let Some((sender_mac, _sender_ip)) = parse_arp_reply(&buf, len) {
                    return Some(sender_mac);
                }
            }
        }
    }
    None
}

/// Build an Ethernet/IPv4/UDP/DNS frame with a specific destination MAC.
pub fn build_dns_frame_with_mac(
    dst_mac: &[u8; 6],
    src_mac: &[u8; 6],
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    dns_payload: &[u8],
    dns_len: usize,
    frame: &mut [u8; 1514],
) -> usize {
    // Ethernet header — unicast to nameserver
    frame[0..6].copy_from_slice(dst_mac);
    frame[6..12].copy_from_slice(src_mac);
    frame[12] = 0x08;
    frame[13] = 0x00;

    // IPv4 header
    let ip_off = 14;
    let ip_total_len = 20 + 8 + dns_len;
    frame[ip_off] = 0x45;
    frame[ip_off + 1] = 0x00;
    frame[ip_off + 2..ip_off + 4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    frame[ip_off + 8] = 64; // TTL
    frame[ip_off + 9] = 17; // protocol = UDP
    frame[ip_off + 12..ip_off + 16].copy_from_slice(src_ip); // src IP = our assigned IP
    frame[ip_off + 16..ip_off + 20].copy_from_slice(dst_ip); // dst = nameserver

    let cksum = ip_checksum(&frame[ip_off..ip_off + 20]);
    frame[ip_off + 10..ip_off + 12].copy_from_slice(&cksum.to_be_bytes());

    // UDP header — ephemeral src port, dst port 53 (DNS)
    let udp_off = ip_off + 20;
    let udp_len = 8 + dns_len;
    frame[udp_off..udp_off + 2].copy_from_slice(&[0x12, 0x34]); // src port = txid (ephemeral)
    frame[udp_off + 2..udp_off + 4].copy_from_slice(&[0x00, 0x35]); // dst port 53
    frame[udp_off + 4..udp_off + 6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    // UDP checksum = 0 (optional)

    // DNS payload
    let dhcp_off = udp_off + 8;
    frame[dhcp_off..dhcp_off + dns_len].copy_from_slice(&dns_payload[..dns_len]);

    dhcp_off + dns_len
}

/// Build an Ethernet/IPv4/UDP/DNS frame into `frame`. Returns total frame length.
pub fn build_dns_frame(
    src_mac: &[u8; 6],
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    dns_payload: &[u8],
    dns_len: usize,
    frame: &mut [u8; 1514],
) -> usize {
    // Ethernet header — broadcast dst for DNS (or use unicast to nameserver)
    frame[0..6].fill(0xFF); // broadcast
    frame[6..12].copy_from_slice(src_mac);
    frame[12] = 0x08;
    frame[13] = 0x00;

    // IPv4 header
    let ip_off = 14;
    let ip_total_len = 20 + 8 + dns_len;
    frame[ip_off] = 0x45;
    frame[ip_off + 1] = 0x00;
    frame[ip_off + 2..ip_off + 4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    frame[ip_off + 8] = 64; // TTL
    frame[ip_off + 9] = 17; // protocol = UDP
    frame[ip_off + 12..ip_off + 16].copy_from_slice(src_ip); // src IP = our assigned IP
    frame[ip_off + 16..ip_off + 20].copy_from_slice(dst_ip); // dst = nameserver

    let cksum = ip_checksum(&frame[ip_off..ip_off + 20]);
    frame[ip_off + 10..ip_off + 12].copy_from_slice(&cksum.to_be_bytes());

    // UDP header — ephemeral src port, dst port 53 (DNS)
    let udp_off = ip_off + 20;
    let udp_len = 8 + dns_len;
    frame[udp_off..udp_off + 2].copy_from_slice(&[0x12, 0x34]); // src port = txid (ephemeral)
    frame[udp_off + 2..udp_off + 4].copy_from_slice(&[0x00, 0x35]); // dst port 53
    frame[udp_off + 4..udp_off + 6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    // UDP checksum = 0 (optional)

    // DNS payload
    let dhcp_off = udp_off + 8;
    frame[dhcp_off..dhcp_off + dns_len].copy_from_slice(&dns_payload[..dns_len]);

    dhcp_off + dns_len
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac() -> [u8; 6] {
        [0x52, 0x54, 0x00, 0x12, 0x35, 0x56]
    }

    #[test]
    fn test_build_discover_header() {
        let p = build_discover(0x12345678, &mac());
        assert_eq!(p[0], 1, "op = BOOTREQUEST");
        assert_eq!(p[1], 1, "htype = Ethernet");
        assert_eq!(p[2], 6, "hlen = 6");
        assert_eq!(p[4..8], [0x12, 0x34, 0x56, 0x78], "xid");
        assert_eq!(p[10], 0x80, "broadcast flag");
        assert_eq!(p[28..34], mac(), "chaddr");
        assert_eq!(p[236..240], [0x63, 0x82, 0x53, 0x63], "magic cookie");
    }

    #[test]
    fn test_build_discover_options() {
        let p = build_discover(0x12345678, &mac());
        // Option 53 (DHCP Message Type) = 1 (DISCOVER)
        assert_eq!(p[240], 53);
        assert_eq!(p[241], 1);
        assert_eq!(p[242], 1);
        // Option 55 (Parameter Request List) = 1,3,6
        assert_eq!(p[243], 55);
        assert_eq!(p[244], 3);
        assert_eq!(p[245..248], [1, 3, 6]);
        // Option 255 (End)
        assert_eq!(p[248], 255);
    }

    #[test]
    fn test_build_discover_padding() {
        let p = build_discover(0x12345678, &mac());
        // Bytes 249..300 should be zero (padding)
        for i in 249..300 {
            assert_eq!(p[i], 0, "padding byte {} should be 0", i);
        }
        assert_eq!(p.len(), 300);
    }

    #[test]
    fn test_ip_checksum_all_ones() {
        // The checksum of all-zeros is 0xFFFF (one's complement of 0)
        // because the checksum field is 0, and ~(0) = 0xFFFF.
        assert_eq!(ip_checksum(&[0u8; 20]), 0xFFFF);
    }

    #[test]
    fn test_ip_checksum_known_value() {
        // Standard test vector: IPv4 header → checksum 0xb1e6
        let hdr: [u8; 20] = [
            0x45, 0x00, 0x00, 0x3c, 0x1c, 0x46, 0x40, 0x00,
            0x40, 0x06, 0x00, 0x00, 0xac, 0x10, 0x0a, 0x63,
            0xac, 0x10, 0x0a, 0x0c,
        ];
        assert_eq!(ip_checksum(&hdr), 0xb1e6);
    }

    #[test]
    fn test_build_eth_ip_udp_frame_layout() {
        let m = mac();
        let p = build_discover(0x12345678, &m);
        let mut frame = [0u8; 1514];
        let len = build_eth_ip_udp(&m, &p, 300, &mut frame);
        // 14 (eth) + 20 (ip) + 8 (udp) + 300 (dhcp) = 342
        assert_eq!(len, 342);

        // Ethernet: broadcast dst, our MAC src, EtherType IPv4
        assert_eq!(&frame[0..6], &[0xFF; 6]);
        assert_eq!(&frame[6..12], &m);
        assert_eq!(frame[12], 0x08);
        assert_eq!(frame[13], 0x00);

        // IPv4: version/ihl, TTL, protocol UDP
        assert_eq!(frame[14], 0x45);
        assert_eq!(frame[22], 64); // TTL
        assert_eq!(frame[23], 17); // UDP

        // IP total length = 20 + 8 + 300 = 328
        assert_eq!(u16::from_be_bytes([frame[16], frame[17]]), 328);

        // UDP: src port 68, dst port 67
        assert_eq!(&frame[34..36], &[0x00, 0x44]);
        assert_eq!(&frame[36..38], &[0x00, 0x43]);
        // UDP length = 8 + 300 = 308
        assert_eq!(u16::from_be_bytes([frame[38], frame[39]]), 308);

        // DHCP payload starts at offset 42
        assert_eq!(frame[42], 1); // BOOTREQUEST
        assert_eq!(frame[43], 1); // htype
    }

    /// Build a minimal OFFER/ACK response frame for testing parse_response.
    /// Returns a fixed-size array (no Vec in no_std).
    fn build_response_frame(xid: u32, m: &[u8; 6], yiaddr: [u8; 4],
                            msg_type: u8, include_subnet: bool, include_gateway: bool,
                            include_nameserver: bool) -> [u8; 342] {
        let mut frame = [0u8; 342];
        let mut pos = 0;
        // Ethernet: dst = us, src = server MAC
        frame[pos..pos + 6].copy_from_slice(m); pos += 6;
        frame[pos..pos + 6].copy_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x35, 0x57]); pos += 6;
        frame[pos..pos + 2].copy_from_slice(&[0x08, 0x00]); pos += 2; // EtherType IPv4
        // IPv4
        let ip_off = pos;
        frame[pos] = 0x45; pos += 1; // version/ihl
        frame[pos] = 0x00; pos += 1; // DSCP/ECN
        let total_len: u16 = 20 + 8 + 300;
        frame[pos..pos + 2].copy_from_slice(&total_len.to_be_bytes()); pos += 2;
        frame[pos..pos + 4].copy_from_slice(&[0; 4]); pos += 4; // id, flags, frag
        frame[pos] = 64; pos += 1; // TTL
        frame[pos] = 17; pos += 1; // protocol = UDP
        frame[pos..pos + 2].copy_from_slice(&[0; 2]); pos += 2; // checksum
        frame[pos..pos + 4].copy_from_slice(&[10, 0, 0, 2]); pos += 4; // src
        frame[pos..pos + 4].copy_from_slice(&yiaddr); pos += 4; // dst
        // Compute IP checksum
        let cksum = ip_checksum(&frame[ip_off..ip_off + 20]);
        frame[ip_off + 10..ip_off + 12].copy_from_slice(&cksum.to_be_bytes());
        // UDP
        frame[pos..pos + 2].copy_from_slice(&[0x00, 0x43]); pos += 2; // src 67
        frame[pos..pos + 2].copy_from_slice(&[0x00, 0x44]); pos += 2; // dst 68
        let udp_len: u16 = total_len - 20;
        frame[pos..pos + 2].copy_from_slice(&udp_len.to_be_bytes()); pos += 2;
        frame[pos..pos + 2].copy_from_slice(&[0; 2]); pos += 2; // checksum
        // DHCP
        let dhcp_off = pos;
        frame[pos] = 2; pos += 1; // op = BOOTREPLY
        frame[pos] = 1; pos += 1; // htype
        frame[pos] = 6; pos += 1; // hlen
        frame[pos] = 0; pos += 1; // hops
        frame[pos..pos + 4].copy_from_slice(&xid.to_be_bytes()); pos += 4;
        frame[pos..pos + 2].copy_from_slice(&[0; 2]); pos += 2; // secs
        frame[pos..pos + 2].copy_from_slice(&[0; 2]); pos += 2; // flags
        frame[pos..pos + 4].copy_from_slice(&[0; 4]); pos += 4; // ciaddr
        frame[pos..pos + 4].copy_from_slice(&yiaddr); pos += 4; // yiaddr
        frame[pos..pos + 4].copy_from_slice(&[0; 4]); pos += 4; // siaddr
        frame[pos..pos + 4].copy_from_slice(&[0; 4]); pos += 4; // giaddr
        // chaddr: 16 bytes total, first 6 are our MAC
        frame[pos..pos + 6].copy_from_slice(m); pos += 6;
        frame[pos..pos + 10].fill(0); pos += 10; // rest of chaddr (16 - 6 = 10)
        // sname: 64 bytes
        frame[pos..pos + 64].fill(0); pos += 64;
        // file: 128 bytes
        frame[pos..pos + 128].fill(0); pos += 128;
        // Magic cookie (at dhcp_off + 236)
        frame[dhcp_off + 236..dhcp_off + 240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
        // Options (at dhcp_off + 240)
        let opt_off = dhcp_off + 240;
        frame[opt_off] = 53; frame[opt_off + 1] = 1; frame[opt_off + 2] = msg_type;
        let mut o = opt_off + 3;
        if include_subnet {
            frame[o] = 1; frame[o + 1] = 4;
            frame[o + 2..o + 6].copy_from_slice(&[255, 255, 255, 0]);
            o += 6;
        }
        if include_gateway {
            frame[o] = 3; frame[o + 1] = 4;
            frame[o + 2..o + 6].copy_from_slice(&[10, 0, 0, 1]);
            o += 6;
        }
        if include_nameserver {
            frame[o] = 6; frame[o + 1] = 4;
            frame[o + 2..o + 6].copy_from_slice(&[8, 8, 8, 8]);
            o += 6;
        }
        frame[o] = 255; // end
        frame
    }

    #[test]
    fn test_parse_response_offer() {
        let m = mac();
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, true, true, false);
        let cfg = parse_response(&frame, frame.len(), 0x12345678, &m).unwrap();
        assert_eq!(cfg.yiaddr, [10, 0, 2, 15]);
        assert_eq!(cfg.subnet, [255, 255, 255, 0]);
        assert_eq!(cfg.gateway, [10, 0, 0, 1]);
        assert_eq!(cfg.nameserver, [0, 0, 0, 0]); // absent → default 0
    }

    #[test]
    fn test_parse_response_ack() {
        let m = mac();
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 5, true, false, false);
        let cfg = parse_response(&frame, frame.len(), 0x12345678, &m).unwrap();
        assert_eq!(cfg.yiaddr, [10, 0, 2, 15]);
        assert_eq!(cfg.subnet, [255, 255, 255, 0]);
        assert_eq!(cfg.gateway, [0, 0, 0, 0]); // absent → default 0
    }

    #[test]
    fn test_parse_response_nameserver() {
        let m = mac();
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 5, true, true, true);
        let cfg = parse_response(&frame, frame.len(), 0x12345678, &m).unwrap();
        assert_eq!(cfg.nameserver, [8, 8, 8, 8]);
    }

    #[test]
    fn test_parse_response_rejects_mismatched_xid() {
        let m = mac();
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, false, false, false);
        assert!(parse_response(&frame, frame.len(), 0xDEADBEEF, &m).is_none());
    }

    #[test]
    fn test_parse_response_rejects_mismatched_mac() {
        let m = mac();
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, false, false, false);
        let other_mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        assert!(parse_response(&frame, frame.len(), 0x12345678, &other_mac).is_none());
    }

    #[test]
    fn test_parse_response_rejects_non_offer_ack() {
        let m = mac();
        // msg_type = 1 (DISCOVER) → not OFFER/ACK → reject
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 1, false, false, false);
        assert!(parse_response(&frame, frame.len(), 0x12345678, &m).is_none());
    }

    #[test]
    fn test_parse_response_rejects_too_short() {
        let m = mac();
        // len < 282 → reject
        assert!(parse_response(&[0u8; 100], 100, 0, &m).is_none());
    }

    #[test]
    fn test_parse_response_rejects_bad_ethertype() {
        let m = mac();
        let mut frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, false, false, false);
        frame[12] = 0x86; // ARP, not IPv4
        assert!(parse_response(&frame, frame.len(), 0x12345678, &m).is_none());
    }

    #[test]
    fn test_parse_response_rejects_non_udp() {
        let m = mac();
        let mut frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, false, false, false);
        frame[23] = 6; // TCP, not UDP
        assert!(parse_response(&frame, frame.len(), 0x12345678, &m).is_none());
    }

    #[test]
    fn test_parse_response_rejects_bad_magic() {
        let m = mac();
        let mut frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, false, false, false);
        // Corrupt magic cookie (at dhcp_off + 236 = 42 + 236 = 278)
        frame[278] = 0xFF;
        assert!(parse_response(&frame, frame.len(), 0x12345678, &m).is_none());
    }

    #[test]
    fn test_encode_domain() {
        let (encoded, _len) = encode_domain("google.com").unwrap();
        assert_eq!(encoded[0], 6); // "google" length
        assert!(&encoded[1..7] == b"google");
        assert_eq!(encoded[7], 3); // "com" length
        assert!(&encoded[8..11] == b"com");
        assert_eq!(encoded[11], 0); // root label
    }

    #[test]
    fn test_build_dns_query_layout() {
        let pkt = build_dns_query("google.com", 0x1234).unwrap();
        // Transaction ID
        assert_eq!(u16::from_be_bytes([pkt[0], pkt[1]]), 0x1234);
        // Flags: standard query, recursion desired
        assert_eq!(u16::from_be_bytes([pkt[2], pkt[3]]), 0x0100);
        // qdcount = 1
        assert_eq!(u16::from_be_bytes([pkt[4], pkt[5]]), 1);
        // ancount, nscount, arcount = 0
        assert_eq!(u16::from_be_bytes([pkt[6], pkt[7]]), 0);
        assert_eq!(u16::from_be_bytes([pkt[8], pkt[9]]), 0);
        assert_eq!(u16::from_be_bytes([pkt[10], pkt[11]]), 0);
        // Question: google.com type A class IN
        let qoff = 12;
        assert_eq!(pkt[qoff], 6);
        assert!(&pkt[qoff + 1..qoff + 7] == b"google");
        assert_eq!(pkt[qoff + 7], 3);
        assert!(&pkt[qoff + 8..qoff + 11] == b"com");
        assert_eq!(pkt[qoff + 11], 0); // root label
        assert_eq!(u16::from_be_bytes([pkt[qoff + 12], pkt[qoff + 13]]), 1); // type A
        assert_eq!(u16::from_be_bytes([pkt[qoff + 14], pkt[qoff + 15]]), 1); // class IN
    }

    /// Build a minimal DNS response frame for testing parse_dns_response.
    fn build_dns_response_frame(txid: u16, src_mac: &[u8; 6], answers: &[[u8; 4]]) -> [u8; 342] {
        let mut frame = [0u8; 342];
        let mut pos = 0;
        // Ethernet: dst = us, src = server
        frame[pos..pos + 6].copy_from_slice(src_mac); pos += 6;
        frame[pos..pos + 6].copy_from_slice(&[0x52, 0x54, 0x00, 0x12, 0x35, 0x57]); pos += 6;
        frame[pos..pos + 2].copy_from_slice(&[0x08, 0x00]); pos += 2;
        // IPv4
        let ip_off = pos;
        frame[pos] = 0x45; pos += 1;
        frame[pos] = 0x00; pos += 1;
        let total_len: u16 = 20 + 8 + 256;
        frame[pos..pos + 2].copy_from_slice(&total_len.to_be_bytes()); pos += 2;
        frame[pos..pos + 4].copy_from_slice(&[0; 4]); pos += 4;
        frame[pos] = 64; pos += 1;
        frame[pos] = 17; pos += 1; // UDP
        frame[pos..pos + 2].copy_from_slice(&[0; 2]); pos += 2; // checksum
        frame[pos..pos + 4].copy_from_slice(&[10, 0, 0, 2]); pos += 4; // src
        frame[pos..pos + 4].copy_from_slice(&[8, 8, 8, 8]); pos += 4; // dst (nameserver)
        let cksum = ip_checksum(&frame[ip_off..ip_off + 20]);
        frame[ip_off + 10..ip_off + 12].copy_from_slice(&cksum.to_be_bytes());
        // UDP
        frame[pos..pos + 2].copy_from_slice(&[0x00, 0x35]); pos += 2; // src 53
        frame[pos..pos + 2].copy_from_slice(&[0x00, 0x35]); pos += 2; // dst 53
        let udp_len: u16 = total_len - 20;
        frame[pos..pos + 2].copy_from_slice(&udp_len.to_be_bytes()); pos += 2;
        frame[pos..pos + 2].copy_from_slice(&[0; 2]); pos += 2; // checksum
        // DNS
        let dns_off = pos;
        frame[dns_off..dns_off + 2].copy_from_slice(&txid.to_be_bytes());
        frame[dns_off + 2] = 0x81; frame[dns_off + 3] = 0x80; // response, no error
        let ancount = answers.len() as u16;
        frame[dns_off + 6..dns_off + 8].copy_from_slice(&ancount.to_be_bytes());
        // Question (same as query, skip the 12-byte header)
        let qoff = dns_off + 12;
        let query = build_dns_query("google.com", txid).unwrap();
        let qlen = query.len();
        let mut actual_q = qlen - 12; // skip header
        for i in ((12)..qlen).rev() {
            if query[i] != 0 { actual_q = i + 1 - 12; break; }
        }
        frame[qoff..qoff + actual_q].copy_from_slice(&query[12..12 + actual_q]);
        // Answers
        let mut aoff = qoff + actual_q;
        for addr in answers {
            // Name: compression pointer to question (offset 0xc = 12)
            frame[aoff] = 0xC0; frame[aoff + 1] = 0x0C; aoff += 2;
            // type A
            frame[aoff] = 0x00; frame[aoff + 1] = 0x01; aoff += 2;
            // class IN
            frame[aoff] = 0x00; frame[aoff + 1] = 0x01; aoff += 2;
            // TTL
            frame[aoff..aoff + 4].copy_from_slice(&[0, 0, 0, 60]); aoff += 4;
            // rdlength = 4
            frame[aoff] = 0x00; frame[aoff + 1] = 0x04; aoff += 2;
            // rdata = IP address
            frame[aoff..aoff + 4].copy_from_slice(addr); aoff += 4;
        }
        frame
    }

    #[test]
    fn test_parse_dns_response_wrong_txid() {
        let m = [0x52, 0x54, 0x00, 0x12, 0x35, 0x56];
        let frame = build_dns_response_frame(0x1234, &m, &[[142, 250, 165, 220]]);
        assert!(parse_dns_response(&frame, frame.len(), 0xDEAD).is_none());
    }

    #[test]
    fn test_parse_dns_response_no_answers() {
        let m = [0x52, 0x54, 0x00, 0x12, 0x35, 0x56];
        let mut frame = build_dns_response_frame(0x1234, &m, &[]);
        // Set ancount to 1 but don't add any answer records
        frame[26] = 0x00; frame[27] = 0x01;
        assert!(parse_dns_response(&frame, frame.len(), 0x1234).is_none());
    }

    #[test]
    fn test_parse_dns_response_too_short() {
        assert!(parse_dns_response(&[0u8; 10], 10, 0x1234).is_none());
    }

    #[test]
    fn test_encode_domain_too_long_label() {
        // Label must be <= 63 bytes; this one is exactly 64
        assert!(encode_domain("a.bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.com").is_none());
    }

    #[test]
    fn test_encode_domain_empty_label() {
        assert!(encode_domain("google..com").is_none());
    }
}
