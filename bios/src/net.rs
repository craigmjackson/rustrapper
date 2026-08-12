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
        print_hex(bar0_u64, 8);
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

/// Establish network connectivity so the Lua REPL `fetch()` builtin can
/// download files from the DHCP TFTP server. Walks PCI for a class-0x02
/// device, initializes its e1000 NIC, runs DHCP, and records the fetch
/// context via [`crate::fetch::set_context`]. Returns `true` on success.
pub fn setup_fetch_context() -> bool {
    puts("Setting up network for fetch()...\n");

    for dev in 0..32u8 {
        let id = pci::pci_read32(0, dev, 0, 0);
        if id == 0xFFFF_FFFF {
            continue;
        }
        let cls = pci::pci_class(0, dev, 0);
        if cls != 0x02 {
            continue;
        }

        pci::pci_enable_bars(0, dev, 0);
        let bar0 = pci::pci_read32(0, dev, 0, 0x10) & !0xF;
        if bar0 == 0 {
            continue;
        }

        let bar0_u64 = bar0 as u64;
        let mac = match e1000_common::init(bar0_u64) {
            Some(mac) => mac,
            None => continue,
        };

        match dhcp_run(bar0_u64, &mac) {
            Some(cfg) => {
                crate::fetch::set_context(bar0_u64, &mac, &cfg);
                puts("  fetch() ready.\n");
                return true;
            }
            None => continue,
        }
    }

    puts("  fetch() unavailable (no network adapter).\n");
    false
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

    // Poll for the DHCP OFFER. dnsmasq runs ARP conflict-detection probes
    // before offering, so the OFFER can arrive well after the DISCOVER. A
    // single `try_receive` window is too short under QEMU TCG (AGENTS.md), so
    // tolerate timeouts instead of aborting on the first one and keep going
    // past ARP/other traffic that parse_response discards.
    let mut buf = [0u8; 1514];
    for _retry in 0..4u32 {
        if let Some(copy_len) = e1000_common::try_receive(base, &mut buf, 100_000_000) {
            if let Some(cfg) = dhcp::parse_response(&buf, copy_len, xid, mac) {
                return Some(cfg);
            }
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
    let mut sink = crate::mem::BiosExtendedMemorySink::new(16 * 1024 * 1024); // 16MB
    
    // Perform TFTP download
    let tftp_result = tftp_download(base, mac, &cfg.yiaddr, &cfg.next_server, filename, &mut sink);
    
    match tftp_result {
        Some(size) => {
            puts("    PXE: Downloaded ");
            print_dec(size as u64);
            puts(" bytes\n");
            
            if filename.ends_with(".lua") {
                puts("    PXE: Executing Lua script\n");
                crate::fetch::set_context(base, mac, cfg);
                let data = unsafe {
                    core::slice::from_raw_parts(sink.buffer_addr() as usize as *const u8, size)
                };
                match lua::run_with_fetch(data, putc, crate::fetch::fetch_file) {
                    Ok(()) => puts("    PXE: Lua script done\n"),
                    Err(e) => {
                        puts("    PXE: Lua error: ");
                        puts(e);
                        putc(b'\n');
                    }
                }
            } else {
                // Execute the downloaded file
                crate::loader::execute_file(
                    sink.buffer_addr() as usize as *mut u8,
                    size,
                    puts,
                );
            }
        }
        None => {
            puts("    PXE: Download failed\n");
        }
    }
}

/// Extract source port from a received Ethernet+IPv4+UDP frame.
/// Returns the UDP source port, or 69 (default TFTP port) if parsing fails.
fn extract_src_port(frame: &[u8], len: usize) -> u16 {
    if len < 42 { return 69; } // Minimum: Eth(14) + IP(20) + UDP(8)
    let ip_hdr_len = ((frame[14] & 0x0F) as usize) * 4;
    let udp_offset = 14 + ip_hdr_len;
    if udp_offset + 4 > len { return 69; }
    u16::from_be_bytes([frame[udp_offset], frame[udp_offset + 1]])
}

/// TFTP download using e1000
pub fn tftp_download(
    base: u64,
    mac: &[u8; 6],
    src_ip: &[u8; 4],
    dst_ip: &[u8; 4],
    filename: &str,
    sink: &mut impl common::tftp::TftpSink,
) -> Option<usize> {
    use common::tftp::*;
    
    // Build RRQ packet
    let mut rrq_buf = [0u8; 512];
    let rrq_len = build_rrq(filename, &mut rrq_buf);
    
    // Build UDP frame
    let mut frame = [0u8; 1514];
    let frame_len = build_udp_frame(mac, src_ip, dst_ip, 68, 69, &rrq_buf[..rrq_len], &mut frame)?;
    
    // Send RRQ
    puts("    TFTP: Sending RRQ...\n");
    if !e1000_common::send(base, &frame[..frame_len]) {
        puts("    TFTP: RRQ send FAILED\n");
        return None;
    }
    puts("    TFTP: RRQ sent, waiting for response...\n");
    
    // Receive and process packets
    let mut server_port: Option<u16> = None; // Will be learned from first response
    let mut _block_num = 0u16;
    let mut blksize = DEFAULT_BLKSIZE;
    let mut total_size = 0usize;
    let mut recv_buf = [0u8; 1514];
    let mut iteration = 0u32;
    
    loop {
        iteration += 1;
        // Receive packet
        let copy_len = match e1000_common::try_receive(base, &mut recv_buf, 100_000_000) {
            Some(len) => len,
            None => {
                puts("    TFTP: receive timeout (try ");
                print_dec(iteration as u64);
                puts(")\n");
                return None;
            }
        };
        
        puts("    TFTP: got ");
        print_dec(copy_len as u64);
        puts(" bytes");
        if copy_len >= 2 {
            puts(", opcode=0x");
            print_hex(u16::from_be_bytes([recv_buf[0], recv_buf[1]]) as u64, 4);
        }
        // Show first 16 bytes of payload for debugging
        puts(" data=");
        let show = copy_len.min(16);
        for i in 0..show {
            let b = recv_buf[i];
            let hi = b >> 4;
            let lo = b & 0x0F;
            putc(if hi < 10 { b'0' + hi } else { b'A' + hi - 10 });
            putc(if lo < 10 { b'0' + lo } else { b'A' + lo - 10 });
        }
        putc(b'\n');
        
        // Respond to ARP requests for our IP
        if copy_len >= 42 && recv_buf[12] == 0x08 && recv_buf[13] == 0x06 {
            let off = 14;
            if recv_buf[off + 6] == 0x00 && recv_buf[off + 7] == 0x01 { // ARP request
                let tpa = [recv_buf[off + 24], recv_buf[off + 25], recv_buf[off + 26], recv_buf[off + 27]];
                if tpa == *src_ip {
                    let mut arp_reply = [0u8; 1514];
                    if let Some(reply_len) = common::arp::build_reply(mac, *src_ip, &recv_buf, &mut arp_reply) {
                        e1000_common::send(base, &arp_reply[..reply_len]);
                    }
                    continue;
                }
            }
        }

        // Learn the server's ephemeral port from the first response
        if server_port.is_none() {
            let port = extract_src_port(&recv_buf, copy_len);
            server_port = Some(port);
            puts("    TFTP: server port=");
            print_dec(port as u64);
            putc(b'\n');
        }
        let sport = server_port.unwrap_or(69);
        
        // Strip Ethernet+IP+UDP headers to get TFTP payload.
        // Bounded by the UDP length field (not the raw frame length) so the
        // 4-byte FCS that QEMU's e1000 appends to RX frames is never parsed
        // as script/binary data.
        let tftp_payload = match udp_payload(&recv_buf, copy_len) {
            Some(p) => p,
            None => continue,
        };
        let tftp_len = tftp_payload.len();
        
        // Detect TFTP ERROR packets (opcode 5)
        if tftp_len >= 4 {
            let opcode = u16::from_be_bytes([tftp_payload[0], tftp_payload[1]]);
            if opcode == 5 { // OP_ERROR
                puts("    TFTP: ERROR from server: ");
                print_dec(u16::from_be_bytes([tftp_payload[2], tftp_payload[3]]) as u64);
                puts(" - ");
                let msg_end = tftp_payload[4..].iter().position(|&b| b == 0).unwrap_or(tftp_len - 4);
                for &b in &tftp_payload[4..4 + msg_end] {
                    putc(b);
                }
                putc(b'\n');
                return None;
            }
        }

        // Check if it's an OACK
        if let Some((new_blksize, _tsize)) = parse_oack(tftp_payload, tftp_len) {
            blksize = new_blksize;
            // Send ACK for block 0 to server's ephemeral port
            let mut ack_buf = [0u8; 4];
            let ack_len = build_ack(0, &mut ack_buf);
            let mut ack_frame = [0u8; 1514];
            if let Some(ack_frame_len) = build_udp_frame(mac, src_ip, dst_ip, 68, sport, &ack_buf[..ack_len], &mut ack_frame) {
                e1000_common::send(base, &ack_frame[..ack_frame_len]);
            }
            continue;
        }
        
        // Check if it's a DATA packet
        if let Some((block, data)) = parse_data(tftp_payload, tftp_len) {
            // Write to sink
            if sink.write_block(data).is_err() {
                return None;
            }
            total_size += data.len();
            _block_num = block;
            
            // Send ACK to server's ephemeral port
            let mut ack_buf = [0u8; 4];
            let ack_len = build_ack(block, &mut ack_buf);
            let mut ack_frame = [0u8; 1514];
            if let Some(ack_frame_len) = build_udp_frame(mac, src_ip, dst_ip, 68, sport, &ack_buf[..ack_len], &mut ack_frame) {
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
