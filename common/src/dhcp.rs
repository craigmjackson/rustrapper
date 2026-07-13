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

/// A successful DHCP result: the IP address we were assigned, plus subnet and gateway.
#[derive(Clone, Copy)]
pub struct DhcpConfig {
    pub yiaddr: [u8; 4],
    pub subnet: [u8; 4],
    pub gateway: [u8; 4],
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
    })
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
                            msg_type: u8, include_subnet: bool, include_gateway: bool) -> [u8; 342] {
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
        frame[o] = 255; // end
        frame
    }

    #[test]
    fn test_parse_response_offer() {
        let m = mac();
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, true, true);
        let cfg = parse_response(&frame, frame.len(), 0x12345678, &m).unwrap();
        assert_eq!(cfg.yiaddr, [10, 0, 2, 15]);
        assert_eq!(cfg.subnet, [255, 255, 255, 0]);
        assert_eq!(cfg.gateway, [10, 0, 0, 1]);
    }

    #[test]
    fn test_parse_response_ack() {
        let m = mac();
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 5, true, false);
        let cfg = parse_response(&frame, frame.len(), 0x12345678, &m).unwrap();
        assert_eq!(cfg.yiaddr, [10, 0, 2, 15]);
        assert_eq!(cfg.subnet, [255, 255, 255, 0]);
        assert_eq!(cfg.gateway, [0, 0, 0, 0]); // absent → default 0
    }

    #[test]
    fn test_parse_response_rejects_mismatched_xid() {
        let m = mac();
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, false, false);
        assert!(parse_response(&frame, frame.len(), 0xDEADBEEF, &m).is_none());
    }

    #[test]
    fn test_parse_response_rejects_mismatched_mac() {
        let m = mac();
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, false, false);
        let other_mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        assert!(parse_response(&frame, frame.len(), 0x12345678, &other_mac).is_none());
    }

    #[test]
    fn test_parse_response_rejects_non_offer_ack() {
        let m = mac();
        // msg_type = 1 (DISCOVER) → not OFFER/ACK → reject
        let frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 1, false, false);
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
        let mut frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, false, false);
        frame[12] = 0x86; // ARP, not IPv4
        assert!(parse_response(&frame, frame.len(), 0x12345678, &m).is_none());
    }

    #[test]
    fn test_parse_response_rejects_non_udp() {
        let m = mac();
        let mut frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, false, false);
        frame[23] = 6; // TCP, not UDP
        assert!(parse_response(&frame, frame.len(), 0x12345678, &m).is_none());
    }

    #[test]
    fn test_parse_response_rejects_bad_magic() {
        let m = mac();
        let mut frame = build_response_frame(0x12345678, &m, [10, 0, 2, 15], 2, false, false);
        // Corrupt magic cookie (at dhcp_off + 236 = 42 + 236 = 278)
        frame[278] = 0xFF;
        assert!(parse_response(&frame, frame.len(), 0x12345678, &m).is_none());
    }
}
