//! ARP request build and reply parse.
//!
//! Used to resolve the MAC address of the next-hop gateway or on-subnet DNS
//! server before sending a unicast DNS query.

/// Build a broadcast ARP request frame (Ethernet + ARP, 42 bytes).
///
/// - `src_mac` / `src_ip`: our MAC and IP
/// - `dst_ip`: the IP we want to resolve
///
/// Returns the total frame length written into `frame`.
pub fn build_request(
    src_mac: &[u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    frame: &mut [u8; 1514],
) -> usize {
    // Ethernet header: broadcast dst, our MAC src, EtherType ARP
    frame[0..6].fill(0xFF);
    frame[6..12].copy_from_slice(src_mac);
    frame[12] = 0x08;
    frame[13] = 0x06;

    // ARP payload (28 bytes)
    let off = 14;
    frame[off] = 0x00;
    frame[off + 1] = 0x01; // htype = Ethernet
    frame[off + 2] = 0x08;
    frame[off + 3] = 0x00; // ptype = IPv4
    frame[off + 4] = 6; // hlen
    frame[off + 5] = 4; // plen
    frame[off + 6] = 0x00;
    frame[off + 7] = 0x01; // oper = request
    frame[off + 8..off + 14].copy_from_slice(src_mac); // sha
    frame[off + 14..off + 18].copy_from_slice(&src_ip); // spa
    frame[off + 18..off + 24].fill(0); // tha = 0
    frame[off + 24..off + 28].copy_from_slice(&dst_ip); // tpa

    off + 28
}

/// Parse an ARP reply. Returns the sender's MAC address if the frame is an
/// ARP reply for `target_ip` (i.e. the sender's protocol address matches).
pub fn parse_reply(buf: &[u8], len: usize, target_ip: [u8; 4]) -> Option<[u8; 6]> {
    if len < 42 {
        return None;
    }
    // EtherType ARP
    if buf[12] != 0x08 || buf[13] != 0x06 {
        return None;
    }
    let off = 14;
    // htype = Ethernet, ptype = IPv4
    if buf[off] != 0x00 || buf[off + 1] != 0x01 || buf[off + 2] != 0x08 || buf[off + 3] != 0x00 {
        return None;
    }
    if buf[off + 4] != 6 || buf[off + 5] != 4 {
        return None;
    }
    // oper = reply
    if buf[off + 6] != 0x00 || buf[off + 7] != 0x02 {
        return None;
    }
    // sender protocol address must match the IP we asked for
    let spa = [buf[off + 14], buf[off + 15], buf[off + 16], buf[off + 17]];
    if spa != target_ip {
        return None;
    }
    let mac: [u8; 6] = [
        buf[off + 8],
        buf[off + 9],
        buf[off + 10],
        buf[off + 11],
        buf[off + 12],
        buf[off + 13],
    ];
    Some(mac)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac() -> [u8; 6] {
        [0x52, 0x54, 0x00, 0x12, 0x35, 0x56]
    }

    #[test]
    fn test_build_request_ethernet_header() {
        let mut frame = [0u8; 1514];
        let len = build_request(&mac(), [10, 0, 2, 15], [10, 0, 2, 2], &mut frame);
        assert_eq!(len, 42);
        // Broadcast destination
        assert_eq!(&frame[0..6], &[0xFF; 6]);
        // Source MAC
        assert_eq!(&frame[6..12], &mac());
        // EtherType ARP
        assert_eq!(frame[12], 0x08);
        assert_eq!(frame[13], 0x06);
    }

    #[test]
    fn test_build_request_arp_fields() {
        let mut frame = [0u8; 1514];
        let len = build_request(&mac(), [10, 0, 2, 15], [10, 0, 2, 2], &mut frame);
        let off = 14;
        assert_eq!(len, 42);
        assert_eq!(&frame[off..off + 2], &[0x00, 0x01]); // htype Ethernet
        assert_eq!(&frame[off + 2..off + 4], &[0x08, 0x00]); // ptype IPv4
        assert_eq!(frame[off + 4], 6); // hlen
        assert_eq!(frame[off + 5], 4); // plen
        assert_eq!(&frame[off + 6..off + 8], &[0x00, 0x01]); // oper request
        assert_eq!(&frame[off + 8..off + 14], &mac()); // sha
        assert_eq!(&frame[off + 14..off + 18], &[10, 0, 2, 15]); // spa
        assert_eq!(&frame[off + 18..off + 24], &[0; 6]); // tha = 0
        assert_eq!(&frame[off + 24..off + 28], &[10, 0, 2, 2]); // tpa
    }

    #[test]
    fn test_parse_reply_valid() {
        let mut frame = [0u8; 1514];
        let _len = build_request(&mac(), [10, 0, 2, 15], [10, 0, 2, 2], &mut frame);
        // Turn it into a reply: swap sender/target, set oper=2
        let off = 14;
        frame[off + 6] = 0x00;
        frame[off + 7] = 0x02; // oper = reply
        // Sender is now 10.0.2.2 with some MAC
        let server_mac = [0x52, 0x54, 0x00, 0x12, 0x35, 0x02];
        frame[off + 8..off + 14].copy_from_slice(&server_mac); // sha
        frame[off + 14..off + 18].copy_from_slice(&[10, 0, 2, 2]); // spa
        // Target is us
        frame[off + 18..off + 24].copy_from_slice(&mac()); // tha
        frame[off + 24..off + 28].copy_from_slice(&[10, 0, 2, 15]); // tpa

        let result = parse_reply(&frame, 42, [10, 0, 2, 2]);
        assert_eq!(result, Some(server_mac));
    }

    #[test]
    fn test_parse_reply_wrong_ip() {
        let mut frame = [0u8; 1514];
        let _len = build_request(&mac(), [10, 0, 2, 15], [10, 0, 2, 2], &mut frame);
        let off = 14;
        frame[off + 7] = 0x02; // oper = reply
        // Reply from wrong IP
        frame[off + 14..off + 18].copy_from_slice(&[10, 0, 2, 99]);

        assert!(parse_reply(&frame, 42, [10, 0, 2, 2]).is_none());
    }

    #[test]
    fn test_parse_reply_not_arp() {
        let mut frame = [0u8; 1514];
        let _len = build_request(&mac(), [10, 0, 2, 15], [10, 0, 2, 2], &mut frame);
        frame[13] = 0x00; // Not ARP
        assert!(parse_reply(&frame, 42, [10, 0, 2, 2]).is_none());
    }

    #[test]
    fn test_parse_reply_request_not_reply() {
        let mut frame = [0u8; 1514];
        let _len = build_request(&mac(), [10, 0, 2, 15], [10, 0, 2, 2], &mut frame);
        // oper is still 1 (request) from build_request
        assert!(parse_reply(&frame, 42, [10, 0, 2, 2]).is_none());
    }

    #[test]
    fn test_parse_reply_too_short() {
        assert!(parse_reply(&[0u8; 30], 30, [10, 0, 2, 2]).is_none());
    }
}
