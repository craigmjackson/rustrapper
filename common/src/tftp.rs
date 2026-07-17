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
    
    // Options (RFC 2348)
    // blksize option
    let blksize_key = b"blksize";
    buf[pos..pos + blksize_key.len()].copy_from_slice(blksize_key);
    pos += blksize_key.len();
    buf[pos] = 0;
    pos += 1;
    
    let blksize_val = b"1468";
    buf[pos..pos + blksize_val.len()].copy_from_slice(blksize_val);
    pos += blksize_val.len();
    buf[pos] = 0;
    pos += 1;
    
    // tsize option (request file size)
    let tsize_key = b"tsize";
    buf[pos..pos + tsize_key.len()].copy_from_slice(tsize_key);
    pos += tsize_key.len();
    buf[pos] = 0;
    pos += 1;
    
    let tsize_val = b"0";
    buf[pos..pos + tsize_val.len()].copy_from_slice(tsize_val);
    pos += tsize_val.len();
    buf[pos] = 0;
    pos += 1;
    
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
        // blksize option
        assert_eq!(&buf[17..25], b"blksize\0");
        assert_eq!(&buf[25..30], b"1468\0");
        // tsize option
        assert_eq!(&buf[30..36], b"tsize\0");
        assert_eq!(&buf[36..38], b"0\0");
        
        assert_eq!(len, 38);
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
}
