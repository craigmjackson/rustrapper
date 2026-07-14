//! DNS query build, unicast UDP frame build, and response parse.
//!
//! - `build_query` encodes a domain name into a standard DNS query
//! - `build_frame` wraps the query in Ethernet + IPv4 + UDP headers for unicast
//! - `parse_response` extracts the first A-record IP from a DNS response

use crate::dhcp::ip_checksum;

/// Build a DNS query (header + single question) into `buf`.
/// Returns the number of bytes written.
///
/// Flags: standard query, recursion desired (0x0100).
/// QTYPE: A (1), QCLASS: IN (1).
pub fn build_query(id: u16, name: &str, buf: &mut [u8; 256]) -> usize {
    // Header (12 bytes)
    buf[0..2].copy_from_slice(&id.to_be_bytes());
    buf[2] = 0x01;
    buf[3] = 0x00; // flags: standard query, recursion desired
    buf[4] = 0x00;
    buf[5] = 0x01; // qdcount = 1
    buf[6..12].fill(0); // ancount, nscount, arcount = 0

    let mut off = 12;
    // Encode QNAME: label length bytes + label data, terminated by 0
    for label in name.split('.') {
        let l = label.len();
        if l == 0 || l > 63 || off + 1 + l >= buf.len() {
            return 0; // name too long or invalid
        }
        buf[off] = l as u8;
        off += 1;
        let bytes = label.as_bytes();
        buf[off..off + l].copy_from_slice(bytes);
        off += l;
    }
    buf[off] = 0; // root terminator
    off += 1;

    // QTYPE = A (1)
    buf[off] = 0x00;
    buf[off + 1] = 0x01;
    // QCLASS = IN (1)
    buf[off + 2] = 0x00;
    buf[off + 3] = 0x01;
    off + 4
}

/// Build the full Ethernet + IPv4 + UDP + DNS frame for unicast transmission.
/// Returns the total frame length written.
pub fn build_frame(
    src_mac: &[u8; 6],
    src_ip: [u8; 4],
    dst_mac: &[u8; 6],
    dst_ip: [u8; 4],
    src_port: u16,
    query: &[u8],
    query_len: usize,
    frame: &mut [u8; 1514],
) -> usize {
    // Ethernet header
    frame[0..6].copy_from_slice(dst_mac);
    frame[6..12].copy_from_slice(src_mac);
    frame[12] = 0x08;
    frame[13] = 0x00; // EtherType IPv4

    // IPv4 header
    let ip_off = 14;
    let ip_total_len = 20 + 8 + query_len;
    frame[ip_off] = 0x45;
    frame[ip_off + 1] = 0x00;
    frame[ip_off + 2..ip_off + 4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    frame[ip_off + 4..ip_off + 8].fill(0); // id, flags, frag
    frame[ip_off + 8] = 64; // TTL
    frame[ip_off + 9] = 17; // protocol = UDP
    frame[ip_off + 10..ip_off + 12].fill(0); // checksum (compute below)
    frame[ip_off + 12..ip_off + 16].copy_from_slice(&src_ip);
    frame[ip_off + 16..ip_off + 20].copy_from_slice(&dst_ip);

    let cksum = ip_checksum(&frame[ip_off..ip_off + 20]);
    frame[ip_off + 10..ip_off + 12].copy_from_slice(&cksum.to_be_bytes());

    // UDP header
    let udp_off = ip_off + 20;
    let udp_len = 8 + query_len;
    frame[udp_off..udp_off + 2].copy_from_slice(&src_port.to_be_bytes());
    frame[udp_off + 2] = 0x00;
    frame[udp_off + 3] = 0x35; // dst port 53
    frame[udp_off + 4..udp_off + 6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    frame[udp_off + 6..udp_off + 8].fill(0); // checksum = 0 (optional)

    // DNS payload
    let dns_off = udp_off + 8;
    frame[dns_off..dns_off + query_len].copy_from_slice(&query[..query_len]);
    dns_off + query_len
}

/// Parse a DNS response frame (Ethernet + IPv4 + UDP + DNS).
/// Returns the first A-record IP address if the response is valid and matches `id`.
pub fn parse_response(buf: &[u8], len: usize, id: u16) -> Option<[u8; 4]> {
    if len < 42 {
        return None;
    }
    // EtherType IPv4
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
    if udp_off + 8 > len {
        return None;
    }
    // Verify dst port is our source port (server replies to our src port)
    let reply_dst_port = u16::from_be_bytes([buf[udp_off + 2], buf[udp_off + 3]]);
    // We can't check the exact port here since the caller might use different ports;
    // just check it's UDP from port 53
    let reply_src_port = u16::from_be_bytes([buf[udp_off], buf[udp_off + 1]]);
    if reply_src_port != 53 {
        return None;
    }

    let dns_off = udp_off + 8;
    if dns_off + 12 > len {
        return None;
    }

    // DNS header
    let pkt_id = u16::from_be_bytes([buf[dns_off], buf[dns_off + 1]]);
    if pkt_id != id {
        return None;
    }
    let flags = u16::from_be_bytes([buf[dns_off + 2], buf[dns_off + 3]]);
    let rcode = (flags & 0x000F) as u8;
    if rcode != 0 {
        return None;
    }
    let qdcount = u16::from_be_bytes([buf[dns_off + 4], buf[dns_off + 5]]) as usize;
    let ancount = u16::from_be_bytes([buf[dns_off + 6], buf[dns_off + 7]]) as usize;
    if ancount == 0 {
        return None;
    }

    let _ = reply_dst_port; // suppress unused warning
    let _ = qdcount;

    // Skip question section
    let mut off = dns_off + 12;
    for _ in 0..qdcount {
        off = skip_name(buf, len, off)?;
        off += 4; // QTYPE + QCLASS
        if off > len {
            return None;
        }
    }

    // Parse answer records — find the first A record
    for _ in 0..ancount {
        off = skip_name(buf, len, off)?;
        if off + 10 > len {
            return None;
        }
        let rtype = u16::from_be_bytes([buf[off], buf[off + 1]]);
        let _rclass = u16::from_be_bytes([buf[off + 2], buf[off + 3]]);
        let _ttl = u32::from_be_bytes([buf[off + 4], buf[off + 5], buf[off + 6], buf[off + 7]]);
        let rdlength = u16::from_be_bytes([buf[off + 8], buf[off + 9]]) as usize;
        off += 10;
        if off + rdlength > len {
            return None;
        }
        if rtype == 1 && rdlength == 4 {
            return Some([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
        }
        off += rdlength;
    }

    None
}

/// Skip a (possibly compressed) DNS name starting at `off`.
/// Returns the offset just past the name, or None on error.
fn skip_name(buf: &[u8], len: usize, mut off: usize) -> Option<usize> {
    loop {
        if off >= len {
            return None;
        }
        let b = buf[off];
        if b == 0 {
            return Some(off + 1);
        }
        if b & 0xC0 == 0xC0 {
            // Compression pointer: 2 bytes, done
            return Some(off + 2);
        }
        // Normal label: skip length byte + label data
        off += 1 + b as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_query_header() {
        let mut buf = [0u8; 256];
        let len = build_query(0xABCD, "google.com", &mut buf);
        assert!(len > 12);
        assert_eq!(&buf[0..2], &[0xAB, 0xCD]); // ID
        assert_eq!(&buf[2..4], &[0x01, 0x00]); // flags: recursion desired
        assert_eq!(&buf[4..6], &[0x00, 0x01]); // qdcount = 1
        assert_eq!(&buf[6..12], &[0; 6]); // ancount, nscount, arcount = 0
    }

    #[test]
    fn test_build_query_name_encoding() {
        let mut buf = [0u8; 256];
        let len = build_query(0x1234, "google.com", &mut buf);
        // QNAME: \x06google\x03com\x00
        assert_eq!(buf[12], 6);
        assert_eq!(&buf[13..19], b"google");
        assert_eq!(buf[19], 3);
        assert_eq!(&buf[20..23], b"com");
        assert_eq!(buf[23], 0); // terminator
        // QTYPE = A (1)
        assert_eq!(&buf[24..26], &[0x00, 0x01]);
        // QCLASS = IN (1)
        assert_eq!(&buf[26..28], &[0x00, 0x01]);
        assert_eq!(len, 28);
    }

    #[test]
    fn test_build_query_single_label() {
        let mut buf = [0u8; 256];
        let len = build_query(0x0001, "localhost", &mut buf);
        assert_eq!(buf[12], 9);
        assert_eq!(&buf[13..22], b"localhost");
        assert_eq!(buf[22], 0);
        assert_eq!(len, 12 + 10 + 1 + 4);
    }

    #[test]
    fn test_build_frame_layout() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x56];
        let dst_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x02];
        let mut query = [0u8; 256];
        let qlen = build_query(0x1234, "google.com", &mut query);
        let mut frame = [0u8; 1514];
        let flen = build_frame(&src_mac, [10, 0, 2, 15], &dst_mac, [10, 0, 2, 2], 0x3039, &query, qlen, &mut frame);

        // Ethernet
        assert_eq!(&frame[0..6], &dst_mac);
        assert_eq!(&frame[6..12], &src_mac);
        assert_eq!(frame[12], 0x08);
        assert_eq!(frame[13], 0x00);
        // IPv4
        assert_eq!(frame[14], 0x45);
        assert_eq!(frame[23], 17); // UDP
        assert_eq!(&frame[26..30], &[10, 0, 2, 15]); // src IP
        assert_eq!(&frame[30..34], &[10, 0, 2, 2]); // dst IP
        // UDP
        assert_eq!(u16::from_be_bytes([frame[34], frame[35]]), 0x3039); // src port
        assert_eq!(u16::from_be_bytes([frame[36], frame[37]]), 53); // dst port
        // DNS starts at offset 42
        assert_eq!(&frame[42..44], &[0x12, 0x34]); // ID
        // Total = 14 + 20 + 8 + qlen
        assert_eq!(flen, 42 + qlen);
    }

    #[test]
    fn test_build_frame_ip_checksum_valid() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x56];
        let dst_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x02];
        let mut query = [0u8; 256];
        let qlen = build_query(0x1234, "g.com", &mut query);
        let mut frame = [0u8; 1514];
        let _ = build_frame(&src_mac, [10, 0, 2, 15], &dst_mac, [10, 0, 2, 2], 0x3039, &query, qlen, &mut frame);
        // Verify IP checksum: computing over the full header (including checksum)
        // should yield 0 (sum is all-1s, one's complement is 0)
        assert_eq!(ip_checksum(&frame[14..34]), 0);
    }

    /// Build a minimal DNS response frame for testing.
    fn build_dns_response(
        id: u16,
        src_mac: &[u8; 6],
        dst_mac: &[u8; 6],
        src_ip: [u8; 4],
        dst_ip: [u8; 4],
        src_port: u16,
        answer_ip: [u8; 4],
        rcode: u8,
        ancount: u16,
    ) -> [u8; 512] {
        let mut frame = [0u8; 512];
        // Ethernet
        frame[0..6].copy_from_slice(dst_mac);
        frame[6..12].copy_from_slice(src_mac);
        frame[12] = 0x08;
        frame[13] = 0x00;
        // IPv4
        let ip_off = 14;
        frame[ip_off] = 0x45;
        let ip_total_len: u16 = 20 + 8 + 12 + 17 + 10 + 4; // header + question + answer
        frame[ip_off + 2..ip_off + 4].copy_from_slice(&ip_total_len.to_be_bytes());
        frame[ip_off + 8] = 64;
        frame[ip_off + 9] = 17;
        frame[ip_off + 12..ip_off + 16].copy_from_slice(&src_ip);
        frame[ip_off + 16..ip_off + 20].copy_from_slice(&dst_ip);
        let cksum = ip_checksum(&frame[ip_off..ip_off + 20]);
        frame[ip_off + 10..ip_off + 12].copy_from_slice(&cksum.to_be_bytes());
        // UDP
        let udp_off = ip_off + 20;
        frame[udp_off..udp_off + 2].copy_from_slice(&src_port.to_be_bytes());
        frame[udp_off + 2..udp_off + 4].copy_from_slice(&0x3039u16.to_be_bytes()); // dst port = our src port
        let udp_len: u16 = 8 + 12 + 17 + 10 + 4;
        frame[udp_off + 4..udp_off + 6].copy_from_slice(&udp_len.to_be_bytes());
        // DNS
        let dns_off = udp_off + 8;
        frame[dns_off..dns_off + 2].copy_from_slice(&id.to_be_bytes());
        let flags: u16 = (0x8000) | (rcode as u16); // QR=1 (response) + rcode
        frame[dns_off + 2..dns_off + 4].copy_from_slice(&flags.to_be_bytes());
        frame[dns_off + 4..dns_off + 6].copy_from_slice(&1u16.to_be_bytes()); // qdcount
        frame[dns_off + 6..dns_off + 8].copy_from_slice(&ancount.to_be_bytes());
        frame[dns_off + 8..dns_off + 12].fill(0);
        // Question: google.com
        let qname = [6, b'g', b'o', b'o', b'g', b'l', b'e', 3, b'c', b'o', b'm', 0];
        let mut off = dns_off + 12;
        frame[off..off + qname.len()].copy_from_slice(&qname);
        off += qname.len();
        frame[off..off + 2].copy_from_slice(&1u16.to_be_bytes()); // QTYPE A
        frame[off + 2..off + 4].copy_from_slice(&1u16.to_be_bytes()); // QCLASS IN
        off += 4;
        // Answer: compressed name pointer to offset 12 (question name)
        if ancount > 0 {
            frame[off] = 0xC0;
            frame[off + 1] = 12; // pointer to question name
            frame[off + 2..off + 4].copy_from_slice(&1u16.to_be_bytes()); // TYPE A
            frame[off + 4..off + 6].copy_from_slice(&1u16.to_be_bytes()); // CLASS IN
            frame[off + 6..off + 10].copy_from_slice(&300u32.to_be_bytes()); // TTL
            frame[off + 10..off + 12].copy_from_slice(&4u16.to_be_bytes()); // RDLENGTH
            frame[off + 12..off + 16].copy_from_slice(&answer_ip);
        }
        let total = off + if ancount > 0 { 16 } else { 0 };
        let _ = total;
        frame
    }

    #[test]
    fn test_parse_response_valid() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x02];
        let dst_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x56];
        let frame = build_dns_response(
            0x1234, &src_mac, &dst_mac, [10, 0, 2, 2], [10, 0, 2, 15], 53,
            [142, 250, 190, 78], 0, 1,
        );
        let ip = parse_response(&frame, frame.len(), 0x1234);
        assert_eq!(ip, Some([142, 250, 190, 78]));
    }

    #[test]
    fn test_parse_response_wrong_id() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x02];
        let dst_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x56];
        let frame = build_dns_response(
            0x1234, &src_mac, &dst_mac, [10, 0, 2, 2], [10, 0, 2, 15], 53,
            [1, 2, 3, 4], 0, 1,
        );
        assert!(parse_response(&frame, frame.len(), 0xDEAD).is_none());
    }

    #[test]
    fn test_parse_response_rcode_error() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x02];
        let dst_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x56];
        let frame = build_dns_response(
            0x1234, &src_mac, &dst_mac, [10, 0, 2, 2], [10, 0, 2, 15], 53,
            [1, 2, 3, 4], 3, 1, // rcode=3 (NXDOMAIN)
        );
        assert!(parse_response(&frame, frame.len(), 0x1234).is_none());
    }

    #[test]
    fn test_parse_response_no_answers() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x02];
        let dst_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x56];
        let frame = build_dns_response(
            0x1234, &src_mac, &dst_mac, [10, 0, 2, 2], [10, 0, 2, 15], 53,
            [1, 2, 3, 4], 0, 0, // ancount=0
        );
        assert!(parse_response(&frame, frame.len(), 0x1234).is_none());
    }

    #[test]
    fn test_parse_response_not_udp() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x02];
        let dst_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x56];
        let mut frame = build_dns_response(
            0x1234, &src_mac, &dst_mac, [10, 0, 2, 2], [10, 0, 2, 15], 53,
            [1, 2, 3, 4], 0, 1,
        );
        frame[23] = 6; // TCP instead of UDP
        assert!(parse_response(&frame, frame.len(), 0x1234).is_none());
    }

    #[test]
    fn test_parse_response_wrong_src_port() {
        let src_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x02];
        let dst_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x56];
        let frame = build_dns_response(
            0x1234, &src_mac, &dst_mac, [10, 0, 2, 2], [10, 0, 2, 15], 9999, // not port 53
            [1, 2, 3, 4], 0, 1,
        );
        assert!(parse_response(&frame, frame.len(), 0x1234).is_none());
    }

    #[test]
    fn test_skip_name_normal() {
        let buf = [6, b'g', b'o', b'o', b'g', b'l', b'e', 3, b'c', b'o', b'm', 0, 0xFF];
        assert_eq!(skip_name(&buf, buf.len(), 0), Some(12));
    }

    #[test]
    fn test_skip_name_pointer() {
        let buf = [0xC0, 0x0C, 0xFF, 0xFF];
        assert_eq!(skip_name(&buf, buf.len(), 0), Some(2));
    }

    #[test]
    fn test_skip_name_root() {
        let buf = [0x00, 0xFF];
        assert_eq!(skip_name(&buf, buf.len(), 0), Some(1));
    }
}
