//! TFTP client implementation (RFC 1350) with block size negotiation (RFC 2348).
//!
//! Supports streaming transfers via the TftpSink trait for large file support.

/// TFTP opcodes
pub const OP_RRQ: u16 = 1;
pub const OP_WRQ: u16 = 2;
pub const OP_DATA: u16 = 3;
pub const OP_ACK: u16 = 4;
pub const OP_ERROR: u16 = 5;
pub const OP_OACK: u16 = 6;

/// Default TFTP block size
pub const DEFAULT_BLKSIZE: usize = 512;

/// Maximum TFTP block size (Ethernet MTU - IP - UDP - overhead)
pub const MAX_BLKSIZE: usize = 1468;

/// TFTP timeout in seconds
pub const TIMEOUT_SECS: u64 = 5;

/// Maximum retries per packet
pub const MAX_RETRIES: u32 = 3;

/// Trait for receiving TFTP data blocks
pub trait TftpSink {
    /// Write a block of data to the sink
    fn write_block(&mut self, data: &[u8]) -> Result<(), ()>;
    
    /// Finalize the transfer with the total size
    fn finalize(&mut self, size: usize) -> Result<(), ()>;
}

/// Build a TFTP RRQ (Read Request) packet with options
pub fn build_rrq(filename: &str, buf: &mut [u8]) -> usize {
    let mut pos = 0;
    
    // Opcode (RRQ = 1)
    buf[pos..pos + 2].copy_from_slice(&OP_RRQ.to_be_bytes());
    pos += 2;
    
    // Filename (null-terminated)
    let filename_bytes = filename.as_bytes();
    let filename_len = filename_bytes.len().min(128);
    buf[pos..pos + filename_len].copy_from_slice(&filename_bytes[..filename_len]);
    pos += filename_len;
    buf[pos] = 0;
    pos += 1;
    
    // Mode (octet = binary)
    let mode = b"octet";
    buf[pos..pos + mode.len()].copy_from_slice(mode);
    pos += mode.len();
    buf[pos] = 0;
    pos += 1;
    
    // Options (RFC 2348) - disabled for compatibility with simple TFTP servers
    // blksize option
    // let blksize_key = b"blksize";
    // buf[pos..pos + blksize_key.len()].copy_from_slice(blksize_key);
    // pos += blksize_key.len();
    // buf[pos] = 0;
    // pos += 1;
    
    // let blksize_val = b"1468";
    // buf[pos..pos + blksize_val.len()].copy_from_slice(blksize_val);
    // pos += blksize_val.len();
    // buf[pos] = 0;
    // pos += 1;
    
    // tsize option (request file size)
    // let tsize_key = b"tsize";
    // buf[pos..pos + tsize_key.len()].copy_from_slice(tsize_key);
    // pos += tsize_key.len();
    // buf[pos] = 0;
    // pos += 1;
    
    // let tsize_val = b"0";
    // buf[pos..pos + tsize_val.len()].copy_from_slice(tsize_val);
    // pos += tsize_val.len();
    // buf[pos] = 0;
    // pos += 1;
    
    pos
}

/// Parse a TFTP OACK (Option Acknowledgment) packet
/// Returns (blksize, tsize) if successful
pub fn parse_oack(buf: &[u8], len: usize) -> Option<(usize, usize)> {
    if len < 2 {
        return None;
    }
    
    let opcode = u16::from_be_bytes([buf[0], buf[1]]);
    if opcode != OP_OACK {
        return None;
    }
    
    let mut blksize = DEFAULT_BLKSIZE;
    let mut tsize = 0;
    let mut pos = 2;
    
    while pos < len {
        // Find key (null-terminated)
        let key_start = pos;
        while pos < len && buf[pos] != 0 {
            pos += 1;
        }
        if pos >= len {
            break;
        }
        let key = &buf[key_start..pos];
        pos += 1; // skip null
        
        // Find value (null-terminated)
        let val_start = pos;
        while pos < len && buf[pos] != 0 {
            pos += 1;
        }
        if pos >= len {
            break;
        }
        let val = &buf[val_start..pos];
        pos += 1; // skip null
        
        // Parse option
        if key == b"blksize" {
            if let Some(size) = parse_decimal(val) {
                blksize = size;
            }
        } else if key == b"tsize" {
            if let Some(size) = parse_decimal(val) {
                tsize = size;
            }
        }
    }
    
    Some((blksize, tsize))
}

/// Parse a TFTP DATA packet
/// Returns (block_number, data_slice) if successful
pub fn parse_data(buf: &[u8], len: usize) -> Option<(u16, &[u8])> {
    if len < 4 {
        return None;
    }
    
    let opcode = u16::from_be_bytes([buf[0], buf[1]]);
    if opcode != OP_DATA {
        return None;
    }
    
    let block = u16::from_be_bytes([buf[2], buf[3]]);
    let data = &buf[4..len];
    
    Some((block, data))
}

/// Build a TFTP ACK packet
pub fn build_ack(block: u16, buf: &mut [u8]) -> usize {
    buf[0..2].copy_from_slice(&OP_ACK.to_be_bytes());
    buf[2..4].copy_from_slice(&block.to_be_bytes());
    4
}

/// Extract the UDP payload from a received Ethernet+IPv4+UDP frame.
///
/// The payload is bounded by the UDP length field rather than the raw frame
/// length: QEMU's e1000 model appends the 4-byte Ethernet FCS to received
/// frames, so the descriptor length reported by `try_receive` is 4 bytes too
/// long on the final packet. Bounding by the UDP length strips any trailing
/// FCS/padding so the payload is never over-read.
pub fn udp_payload<'a>(frame: &'a [u8], len: usize) -> Option<&'a [u8]> {
    if len < 42 {
        return None;
    }
    if frame[12] != 0x08 || frame[13] != 0x00 {
        return None;
    }
    let ip_hdr_len = ((frame[14] & 0x0F) as usize) * 4;
    let udp_offset = 14 + ip_hdr_len;
    if udp_offset + 8 > len {
        return None;
    }
    let udp_len = u16::from_be_bytes([frame[udp_offset + 4], frame[udp_offset + 5]]) as usize;
    let payload_off = udp_offset + 8;
    let payload_end = (udp_offset + udp_len).min(len);
    if payload_off > payload_end {
        return None;
    }
    Some(&frame[payload_off..payload_end])
}

/// Build an Ethernet+IPv4+UDP frame carrying `payload`.
///
/// Destination MAC is broadcast (the caller resolves the real next-hop MAC
/// via ARP when needed); the IP header checksum is computed, the UDP checksum
/// is left zero (acceptable on local links for TFTP/DHCP traffic).
pub fn build_udp_frame(
    src_mac: &[u8; 6],
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
    frame: &mut [u8; 1514],
) -> Option<usize> {
    // Ethernet header
    frame[0..6].copy_from_slice(&[0xff; 6]); // broadcast
    frame[6..12].copy_from_slice(src_mac);
    frame[12..14].copy_from_slice(&0x0800u16.to_be_bytes()); // IPv4

    // IPv4 header
    let ip_hdr_len = 20;
    let udp_hdr_len = 8;
    let total_len = ip_hdr_len + udp_hdr_len + payload.len();

    frame[14] = 0x45; // version 4, IHL 5
    frame[15] = 0x00; // DSCP/ECN
    frame[16..18].copy_from_slice(&(total_len as u16).to_be_bytes());
    frame[18..20].copy_from_slice(&0u16.to_be_bytes()); // identification
    frame[20..22].copy_from_slice(&0x4000u16.to_be_bytes()); // flags/fragment
    frame[22] = 64; // TTL
    frame[23] = 17; // UDP protocol
    frame[24..26].copy_from_slice(&0u16.to_be_bytes()); // checksum placeholder
    frame[26..30].copy_from_slice(src_ip);
    frame[30..34].copy_from_slice(dst_ip);

    // Compute IP header checksum
    let mut sum = 0u32;
    for i in (14..14 + ip_hdr_len).step_by(2) {
        sum += u16::from_be_bytes([frame[i], frame[i + 1]]) as u32;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    let cksum = !(sum as u16);
    frame[24..26].copy_from_slice(&cksum.to_be_bytes());

    // UDP header
    frame[34..36].copy_from_slice(&src_port.to_be_bytes());
    frame[36..38].copy_from_slice(&dst_port.to_be_bytes());
    frame[38..40].copy_from_slice(&((udp_hdr_len + payload.len()) as u16).to_be_bytes());
    frame[40..42].copy_from_slice(&0u16.to_be_bytes()); // UDP checksum (skip for now)

    // Payload
    frame[42..42 + payload.len()].copy_from_slice(payload);

    Some(14 + total_len)
}

/// Parse a decimal number from a byte slice
fn parse_decimal(buf: &[u8]) -> Option<usize> {
    let mut result = 0usize;
    for &byte in buf {
        if byte >= b'0' && byte <= b'9' {
            result = result * 10 + (byte - b'0') as usize;
        } else {
            return None;
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_build_rrq() {
        let mut buf = [0u8; 256];
        let len = build_rrq("test.txt", &mut buf);
        
        // Opcode
        assert_eq!(&buf[0..2], &[0, 1]);
        // Filename
        assert_eq!(&buf[2..11], b"test.txt\0");
        // Mode
        assert_eq!(&buf[11..17], b"octet\0");
        // No options (disabled for compatibility)
        
        assert_eq!(len, 17);
    }
    
    #[test]
    fn test_parse_oack() {
        let mut buf = [0u8; 64];
        buf[0..2].copy_from_slice(&OP_OACK.to_be_bytes());
        let mut pos = 2;
        
        // blksize=1024
        buf[pos..pos + 8].copy_from_slice(b"blksize\0");
        pos += 8;
        buf[pos..pos + 5].copy_from_slice(b"1024\0");
        pos += 5;
        
        // tsize=12345
        buf[pos..pos + 6].copy_from_slice(b"tsize\0");
        pos += 6;
        buf[pos..pos + 6].copy_from_slice(b"12345\0");
        pos += 6;
        
        let (blksize, tsize) = parse_oack(&buf, pos).unwrap();
        assert_eq!(blksize, 1024);
        assert_eq!(tsize, 12345);
    }
    
    #[test]
    fn test_parse_data() {
        let mut buf = [0u8; 516];
        buf[0..2].copy_from_slice(&OP_DATA.to_be_bytes());
        buf[2..4].copy_from_slice(&42u16.to_be_bytes());
        buf[4..9].copy_from_slice(b"hello");
        
        let (block, data) = parse_data(&buf, 9).unwrap();
        assert_eq!(block, 42);
        assert_eq!(data, b"hello");
    }
    
    #[test]
    fn test_build_ack() {
        let mut buf = [0u8; 4];
        let len = build_ack(42, &mut buf);
        
        assert_eq!(&buf[0..2], &[0, 4]); // ACK opcode
        assert_eq!(&buf[2..4], &[0, 42]); // block 42
        assert_eq!(len, 4);
    }
    
    #[test]
    fn test_parse_decimal() {
        assert_eq!(parse_decimal(b"0"), Some(0));
        assert_eq!(parse_decimal(b"123"), Some(123));
        assert_eq!(parse_decimal(b"1468"), Some(1468));
        assert_eq!(parse_decimal(b"abc"), None);
        assert_eq!(parse_decimal(b"12a3"), None);
    }

    /// Build a minimal Ethernet+IPv4+UDP frame with the given UDP payload.
    fn build_test_frame(payload: &[u8]) -> [u8; 1514] {
        let mut frame = [0u8; 1514];
        build_udp_frame(
            &[0x52, 0x54, 0x00, 0x12, 0x34, 0x5b],
            &[10, 0, 2, 15],
            &[10, 0, 2, 2],
            68,
            69,
            payload,
            &mut frame,
        );
        frame
    }

    #[test]
    fn test_build_udp_frame() {
        let payload = b"hello";
        let frame = build_test_frame(payload);

        // Ethernet: broadcast dst, src MAC, IPv4 ethertype
        assert_eq!(&frame[0..6], &[0xff; 6]);
        assert_eq!(&frame[6..12], &[0x52, 0x54, 0x00, 0x12, 0x34, 0x5b]);
        assert_eq!(&frame[12..14], &[0x08, 0x00]);

        // IPv4: version/IHL, total length, protocol UDP
        assert_eq!(frame[14], 0x45);
        assert_eq!(frame[23], 17);
        assert_eq!(&frame[16..18], &((20 + 8 + payload.len()) as u16).to_be_bytes());

        // IP header checksum must be valid (sum of 16-bit words folds to 0)
        let mut sum = 0u32;
        for i in (14..34).step_by(2) {
            sum += u16::from_be_bytes([frame[i], frame[i + 1]]) as u32;
        }
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }
        assert_eq!(sum as u16, 0xFFFF);

        // UDP: src/dst ports, length, payload placement
        assert_eq!(&frame[34..36], &68u16.to_be_bytes());
        assert_eq!(&frame[36..38], &69u16.to_be_bytes());
        assert_eq!(&frame[38..40], &((8 + payload.len()) as u16).to_be_bytes());
        assert_eq!(&frame[42..42 + payload.len()], payload);
    }

    #[test]
    fn test_udp_payload_strips_fcs() {
        let payload = b"TFTPDATA";
        let mut frame = build_test_frame(payload);
        // Total wire length: eth(14) + ip(20) + udp(8) + payload
        let wire_len = 42 + payload.len();
        // QEMU e1000 appends the 4-byte FCS to the RX descriptor length
        let fcs_len = wire_len + 4;
        frame[wire_len] = 0xDE;
        frame[wire_len + 1] = 0xAD;
        frame[wire_len + 2] = 0xBE;
        frame[wire_len + 3] = 0xEF;

        // udp_payload must exclude the trailing FCS bytes
        assert_eq!(udp_payload(&frame, fcs_len), Some(payload as &[u8]));
        // Exact-length frame also parses
        assert_eq!(udp_payload(&frame, wire_len), Some(payload as &[u8]));
    }

    #[test]
    fn test_udp_payload_rejects_bad_frames() {
        // Non-IPv4 ethertype
        let mut arp_frame = build_test_frame(b"x");
        arp_frame[12] = 0x08;
        arp_frame[13] = 0x06;
        assert_eq!(udp_payload(&arp_frame, 100), None);
        // Too short to hold Ethernet+IP+UDP headers
        let ipv4_frame = build_test_frame(b"x");
        assert_eq!(udp_payload(&ipv4_frame, 30), None);
    }
}
