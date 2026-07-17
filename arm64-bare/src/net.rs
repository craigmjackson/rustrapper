use common::dhcp::{self, DhcpConfig};
use common::e1000 as e1000_common;
use common::print::{print_dec, print_hex, putc, puts};

use crate::pci;

/// Walk PCI for a class-0x02 (network) device, initialize its e1000 NIC at
/// BAR0, and run a single-transmit DHCP DISCOVER. All targets' `scan_network`
/// reduce to this one function — the only differences per target are the PCI
/// config-space access (`pci::pci_read32`) and the output sink, both of which
/// are abstracted by the common/e1000 and common/print modules.
pub fn scan_network() {
    puts("Scanning for network adapters...\n");

    for dev in 0..32u8 {
        let id = pci::pci_read32(0, dev, 0, 0);
        if id == 0xFFFF_FFFF {
            continue;
        }
        let cls = pci::pci_class(0, dev, 0);
        if cls != 0x02 {
            continue;
        }

        puts("  PCI device ");
        print_dec(dev as u64);
        puts(": vendor=0x");
        print_hex(pci::pci_vid(0, dev, 0) as u64, 4);
        puts(" device=0x");
        print_hex(pci::pci_did(0, dev, 0) as u64, 4);
        puts("\n");

        puts("    Enabling PCI device...\n");
        pci::pci_enable_bars(0, dev, 0);

        let bar0 = pci::pci_read32(0, dev, 0, 0x10) & !0xF;
        if bar0 == 0 {
            puts("    BAR0 = 0!\n");
            continue;
        }

        let bar0_u64 = bar0 as u64;
        puts("    MMIO BAR0: 0x");
        print_hex(bar0_u64, 16);
        putc(b'\n');

        let mac = match e1000_common::init(bar0_u64) {
            Some(mac) => mac,
            None => {
                puts("    e1000 init failed\n");
                continue;
            }
        };

        puts("    MAC: ");
        print_mac(&mac);
        putc(b'\n');

        puts("    DHCP: ");
        match dhcp_run(bar0_u64, &mac) {
            Some(cfg) => {
                puts("OK\n");
                puts("    IP: ");
                print_ip(&cfg.yiaddr);
                putc(b'\n');
                puts("    Subnet: ");
                print_ip(&cfg.subnet);
                putc(b'\n');
                puts("    Gateway: ");
                if cfg.gateway == [0, 0, 0, 0] {
                    puts("(none)");
                } else {
                    print_ip(&cfg.gateway);
                }
                putc(b'\n');

                common::netio::dns_resolve_and_print(bar0_u64, &mac, &cfg);
                
                // PXE boot: download and execute if next_server and bootfile are present
                if cfg.next_server != [0, 0, 0, 0] && cfg.bootfile[0] != 0 {
                    pxe_boot(bar0_u64, &mac, &cfg);
                }
            }
            None => {
                puts("FAILED\n");
            }
        }

        return;
    }

    puts("  No network adapters found.\n");
}

/// Print a MAC address as `XX:XX:XX:XX:XX:XX` to the global print sink.
fn print_mac(mac: &[u8; 6]) {
    for i in 0..6 {
        if i > 0 {
            putc(b':');
        }
        let hi = mac[i] >> 4;
        let lo = mac[i] & 0x0F;
        putc(if hi < 10 { b'0' + hi } else { b'A' + hi - 10 });
        putc(if lo < 10 { b'0' + lo } else { b'A' + lo - 10 });
    }
}

/// Print an IPv4 address as `A.B.C.D` to the global print sink.
fn print_ip(ip: &[u8; 4]) {
    print_dec(ip[0] as u64);
    putc(b'.');
    print_dec(ip[1] as u64);
    putc(b'.');
    print_dec(ip[2] as u64);
    putc(b'.');
    print_dec(ip[3] as u64);
}

/// Build a DISCOVER, send it, and poll for an OFFER.
/// Returns the assigned IP/subnet/gateway on success.
fn dhcp_run(base: u64, mac: &[u8; 6]) -> Option<DhcpConfig> {
    let xid: u32 = 0x12345678;

    puts("DHCP...");
    let discover = dhcp::build_discover(xid, mac);
    let mut frame = [0u8; 1514];
    let frame_len = dhcp::build_eth_ip_udp(mac, &discover, 300, &mut frame);
    if !e1000_common::send(base, &frame[..frame_len]) {
        puts("send failed\n");
        return None;
    }
    puts("sent ");

    let mut buf = [0u8; 1514];
    for _retry in 0..4u32 {
        let copy_len = e1000_common::try_receive(base, &mut buf, 25_000_000)?;
        if let Some(cfg) = dhcp::parse_response(&buf, copy_len, xid, mac) {
            return Some(cfg);
        }
    }
    None
}

/// PXE boot: download file via TFTP and execute it
fn pxe_boot(base: u64, mac: &[u8; 6], cfg: &DhcpConfig) {
    // Extract filename from bootfile (null-terminated)
    let filename_len = cfg.bootfile.iter().position(|&b| b == 0).unwrap_or(128);
    let filename = match core::str::from_utf8(&cfg.bootfile[..filename_len]) {
        Ok(s) => s,
        Err(_) => {
            puts("    PXE: Invalid bootfile name\n");
            return;
        }
    };
    
    puts("    PXE: Downloading ");
    puts(filename);
    puts(" from ");
    print_ip(&cfg.next_server);
    puts("...\n");
    
    // Allocate memory for the file
    let mut sink = crate::mem::Arm64MemorySink::new(16 * 1024 * 1024); // 16MB
    
    // Perform TFTP download
    let tftp_result = tftp_download(base, mac, &cfg.yiaddr, &cfg.next_server, filename, &mut sink);
    
    match tftp_result {
        Some(size) => {
            puts("    PXE: Downloaded ");
            print_dec(size as u64);
            puts(" bytes\n");
            
            // Execute the downloaded file
            crate::loader::execute_file(
                sink.buffer_addr() as *mut u8,
                size,
                puts,
            );
        }
        None => {
            puts("    PXE: Download failed\n");
        }
    }
}

/// TFTP download using e1000
fn tftp_download(
    base: u64,
    mac: &[u8; 6],
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    filename: &str,
    sink: &mut crate::mem::Arm64MemorySink,
) -> Option<usize> {
    use common::tftp::*;
    
    // Build RRQ packet
    let mut rrq_buf = [0u8; 512];
    let rrq_len = build_rrq(filename, &mut rrq_buf);
    
    // Build UDP frame
    let mut frame = [0u8; 1514];
    let frame_len = build_udp_frame(mac, src_ip, dst_ip, 68, 69, &rrq_buf[..rrq_len], &mut frame)?;
    
    // Send RRQ
    if !e1000_common::send(base, &frame[..frame_len]) {
        return None;
    }
    
    // Receive and process packets
    let mut _block_num = 0u16;
    let mut blksize = DEFAULT_BLKSIZE;
    let mut total_size = 0usize;
    let mut recv_buf = [0u8; 1514];
    
    loop {
        // Receive packet
        let copy_len = e1000_common::try_receive(base, &mut recv_buf, 100_000_000)?;
        
        // Check if it's an OACK
        if let Some((new_blksize, _tsize)) = parse_oack(&recv_buf[..copy_len], copy_len) {
            blksize = new_blksize;
            // Send ACK for block 0
            let mut ack_buf = [0u8; 4];
            let ack_len = build_ack(0, &mut ack_buf);
            let mut ack_frame = [0u8; 1514];
            if let Some(ack_frame_len) = build_udp_frame(mac, src_ip, dst_ip, 68, 69, &ack_buf[..ack_len], &mut ack_frame) {
                e1000_common::send(base, &ack_frame[..ack_frame_len]);
            }
            continue;
        }
        
        // Check if it's a DATA packet
        if let Some((block, data)) = parse_data(&recv_buf[..copy_len], copy_len) {
            // Write to sink
            if sink.write_block(data).is_err() {
                return None;
            }
            total_size += data.len();
            _block_num = block;
            
            // Send ACK
            let mut ack_buf = [0u8; 4];
            let ack_len = build_ack(block, &mut ack_buf);
            let mut ack_frame = [0u8; 1514];
            if let Some(ack_frame_len) = build_udp_frame(mac, src_ip, dst_ip, 68, 69, &ack_buf[..ack_len], &mut ack_frame) {
                e1000_common::send(base, &ack_frame[..ack_frame_len]);
            }
            
            // Check if this was the last block
            if data.len() < blksize {
                break;
            }
        }
    }
    
    sink.finalize(total_size).ok()?;
    Some(total_size)
}

/// Build a UDP frame for TFTP
fn build_udp_frame(
    src_mac: &[u8; 6],
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    src_port: u16,
    dst_port: u16,
    payload: &[u8],
    frame: &mut [u8; 1514],
) -> Option<usize> {
    // Ethernet header
    frame[0..6].copy_from_slice(&[0xff; 6]); // broadcast for now
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
    frame[24..26].copy_from_slice(&0u16.to_be_bytes()); // checksum (skip for now)
    frame[26..30].copy_from_slice(src_ip);
    frame[30..34].copy_from_slice(dst_ip);
    
    // UDP header
    frame[34..36].copy_from_slice(&src_port.to_be_bytes());
    frame[36..38].copy_from_slice(&dst_port.to_be_bytes());
    frame[38..40].copy_from_slice(&((udp_hdr_len + payload.len()) as u16).to_be_bytes());
    frame[40..42].copy_from_slice(&0u16.to_be_bytes()); // checksum (skip for now)
    
    // Payload
    frame[42..42 + payload.len()].copy_from_slice(payload);
    
    Some(14 + total_len)
}
