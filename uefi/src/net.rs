use core::ffi::c_void;

use crate::efi::*;

type LocateHandleBufferFn = unsafe extern "efiapi" fn(
    search_type: u32,
    protocol: *const EFI_GUID,
    search_key: *mut c_void,
    handle_count: *mut UINTN,
    buffer: *mut *mut EFI_HANDLE,
) -> EFI_STATUS;

type OpenProtocolFn = unsafe extern "efiapi" fn(
    handle: EFI_HANDLE,
    protocol: *const EFI_GUID,
    interface: *mut *mut c_void,
    agent_handle: EFI_HANDLE,
    controller_handle: EFI_HANDLE,
    attributes: u32,
) -> EFI_STATUS;

type FreePoolFn = unsafe extern "efiapi" fn(buffer: *mut c_void) -> EFI_STATUS;

const BOOT_SVC_LOCATE_HANDLE_BUFFER: usize = 0x138;
const BOOT_SVC_OPEN_PROTOCOL: usize = 0x118;
const BOOT_SVC_FREE_POOL: usize = 0x48;

fn read_boot_svc_fn<T>(gbs: *const c_void, offset: usize) -> T {
    let ptr = (gbs as usize + offset) as *const *const c_void;
    unsafe { core::mem::transmute_copy(&*ptr) }
}

fn w16(con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL, s: &str) {
    let mut buf = [0u16; 256];
    let bytes = s.as_bytes();
    let len = bytes.len().min(255);
    let mut i = 0;
    while i < len {
        buf[i] = bytes[i] as u16;
        i += 1;
    }
    buf[i] = 0;
    unsafe {
        (con_out.output_string)(con_out as *const _ as *mut _, buf.as_ptr());
    }
}

fn put_dec(con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL, val: u64) {
    let mut rev = [0u16; 24];
    let mut n = 0usize;
    let mut v = val;
    if v == 0 {
        rev[n] = b'0' as u16;
        n = 1;
    } else {
        while v > 0 {
            rev[n] = (b'0' as u16) + (v % 10) as u16;
            v /= 10;
            n += 1;
        }
    }
    let mut buf = [0u16; 24];
    let mut j = 0usize;
    while n > 0 {
        n -= 1;
        buf[j] = rev[n];
        j += 1;
    }
    buf[j] = 0;
    unsafe {
        (con_out.output_string)(con_out as *const _ as *mut _, buf.as_ptr());
    }
}

fn ip_checksum(buf: &[u8]) -> u16 {
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

struct DhcpConfig {
    yiaddr: [u8; 4],
    subnet: [u8; 4],
    gateway: [u8; 4],
}

fn make_dhcp_discover(xid: u32, mac: &[u8; 6]) -> [u8; 300] {
    let mut pkt = [0u8; 300];
    // DHCP header
    pkt[0] = 1;  // op = BOOTREQUEST
    pkt[1] = 1;  // htype = Ethernet
    pkt[2] = 6;  // hlen
    pkt[4..8].copy_from_slice(&xid.to_be_bytes());
    pkt[10] = 0x80;  // flags: broadcast

    pkt[12..16].fill(0); // ciaddr
    pkt[16..20].fill(0); // yiaddr
    pkt[20..24].fill(0); // siaddr
    pkt[24..28].fill(0); // giaddr

    // chaddr
    pkt[28..34].copy_from_slice(mac);
    // sname (64 bytes), file (128 bytes) - already zero

    // DHCP magic cookie
    pkt[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);

    let mut off = 240;

    // DHCP message type: DISCOVER
    pkt[off] = 53;  pkt[off+1] = 1;  pkt[off+2] = 1;
    off += 3;

    // Parameter request list: subnet mask (1), router (3), DNS (6)
    pkt[off] = 55;  pkt[off+1] = 3;  pkt[off+2] = 1;
    pkt[off+3] = 3;  pkt[off+4] = 6;
    off += 5;

    // End
    pkt[off] = 255;

    // Pad remaining to 300 bytes
    let mut pad = off + 1;
    while pad < 300 {
        pkt[pad] = 0;
        pad += 1;
    }

    pkt
}

fn build_eth_ip_udp_dhcp(
    mac: &[u8; 6],
    dhcp_payload: &[u8; 300],
    dhcp_len: usize,
    buf: &mut [u8; 1514],
) -> usize {
    // Ethernet header
    buf[0..6].fill(0xFF);  // dest = broadcast
    buf[6..12].copy_from_slice(mac);  // src = our MAC
    buf[12] = 0x08; buf[13] = 0x00;  // EtherType IPv4

    // IP header at offset 14
    let ip_off = 14usize;
    let ip_total_len = 20 + 8 + dhcp_len;  // IP header + UDP header + DHCP payload

    buf[ip_off] = 0x45;       // Version 4, IHL = 5 (20 bytes)
    buf[ip_off+1] = 0x00;     // DSCP/ECN
    buf[ip_off+2..ip_off+4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    buf[ip_off+4..ip_off+6].copy_from_slice(&[0x00, 0x00]);  // ID
    buf[ip_off+6..ip_off+8].copy_from_slice(&[0x00, 0x00]);  // flags/frag offset
    buf[ip_off+8] = 64;       // TTL
    buf[ip_off+9] = 17;       // UDP protocol
    buf[ip_off+10..ip_off+12].copy_from_slice(&[0x00, 0x00]);  // checksum placeholder
    buf[ip_off+12..ip_off+16].fill(0x00);  // src IP = 0.0.0.0
    buf[ip_off+16..ip_off+20].fill(0xFF);   // dst IP = 255.255.255.255

    // IP checksum
    let cksum = ip_checksum(&buf[ip_off..ip_off+20]);
    buf[ip_off+10..ip_off+12].copy_from_slice(&cksum.to_be_bytes());

    // UDP header at offset 14 + 20 = 34
    let udp_off = ip_off + 20;
    let udp_len = 8 + dhcp_len;
    buf[udp_off..udp_off+2].copy_from_slice(&[0x00, 0x44]);  // src port 68
    buf[udp_off+2..udp_off+4].copy_from_slice(&[0x00, 0x43]); // dst port 67
    buf[udp_off+4..udp_off+6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    buf[udp_off+6..udp_off+8].copy_from_slice(&[0x00, 0x00]); // UDP checksum = 0

    // DHCP payload at offset 14 + 20 + 8 = 42
    let dhcp_off = udp_off + 8;
    buf[dhcp_off..dhcp_off + dhcp_len].copy_from_slice(&dhcp_payload[..dhcp_len]);

    dhcp_off + dhcp_len
}

fn parse_dhcp_response(buf: &[u8], len: usize, xid: u32, mac: &[u8; 6]) -> Option<DhcpConfig> {
    // Minimum: ETH(14) + IP(20) + UDP(8) + DHCP header(240+)
    if len < 282 { return None; }

    // Check EtherType
    if buf[12] != 0x08 || buf[13] != 0x00 { return None; }

    // Check IP dst is broadcast or our IP (but we don't have an IP yet, so just check we can parse it)
    let ip_off = 14;
    let ip_hdr_len = (buf[ip_off] & 0x0F) as usize * 4;
    if ip_hdr_len < 20 { return None; }

    // Check protocol is UDP
    if buf[ip_off + 9] != 17 { return None; }

    let udp_off = ip_off + ip_hdr_len;
    // DHCP payload starts after UDP header
    let dhcp_off = udp_off + 8;
    let dhcp_len = len - dhcp_off;
    if dhcp_len < 240 { return None; }

    // Check DHCP magic cookie
    if dhcp_off + 4 > len { return None; }
    if buf[dhcp_off + 236] != 0x63 || buf[dhcp_off + 237] != 0x82
        || buf[dhcp_off + 238] != 0x53 || buf[dhcp_off + 239] != 0x63 {
        return None;
    }

    // Check xid
    let pkt_xid = u32::from_be_bytes([
        buf[dhcp_off + 4], buf[dhcp_off + 5],
        buf[dhcp_off + 6], buf[dhcp_off + 7],
    ]);
    if pkt_xid != xid { return None; }

    // Check chaddr matches our MAC
    let mut match_mac = true;
    for i in 0..6 {
        if buf[dhcp_off + 28 + i] != mac[i] { match_mac = false; break; }
    }
    if !match_mac { return None; }

    // Read yiaddr (offered IP)
    let yiaddr: [u8; 4] = [
        buf[dhcp_off + 16], buf[dhcp_off + 17],
        buf[dhcp_off + 18], buf[dhcp_off + 19],
    ];

    // Parse DHCP options starting at offset 240
    let mut subnet = [255u8; 4];
    let mut gateway = [0u8; 4];
    let mut dhcp_msg_type = 0u8;
    let mut off = dhcp_off + 240;

    while off + 1 < len {
        let opt_type = buf[off];
        if opt_type == 255 { break; }  // End
        let opt_len = buf[off + 1] as usize;
        if off + 2 + opt_len > len { break; }

        if opt_type == 53 && opt_len == 1 {
            dhcp_msg_type = buf[off + 2];
        } else if opt_type == 1 && opt_len == 4 {
            subnet.copy_from_slice(&buf[off+2..off+6]);
        } else if opt_type == 3 && opt_len >= 4 {
            gateway.copy_from_slice(&buf[off+2..off+6]);
        }

        off += 2 + opt_len;
    }

    // Accept OFFER (2) or ACK (5)
    if dhcp_msg_type != 2 && dhcp_msg_type != 5 { return None; }

    Some(DhcpConfig {
        yiaddr,
        subnet,
        gateway,
    })
}

fn print_mac(con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL, mac: &[u8; 6]) {
    for i in 0..6 {
        if i > 0 { w16(con_out, ":"); }
        let hex = [
            if mac[i] >> 4 < 10 { b'0' + (mac[i] >> 4) } else { b'A' + (mac[i] >> 4) - 10 },
            if mac[i] & 0x0F < 10 { b'0' + (mac[i] & 0x0F) } else { b'A' + (mac[i] & 0x0F) - 10 },
        ];
        let ws = [hex[0] as u16, hex[1] as u16, 0];
        unsafe {
            (con_out.output_string)(con_out as *const _ as *mut _, ws.as_ptr());
        }
    }
}

fn print_ip(con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL, ip: &[u8; 4]) {
    put_dec(con_out, ip[0] as u64);
    w16(con_out, ".");
    put_dec(con_out, ip[1] as u64);
    w16(con_out, ".");
    put_dec(con_out, ip[2] as u64);
    w16(con_out, ".");
    put_dec(con_out, ip[3] as u64);
}

fn print_network_info(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    gbs: *mut c_void,
    image_handle: EFI_HANDLE,
) {
    let locate_handle_buffer: LocateHandleBufferFn =
        read_boot_svc_fn(gbs, BOOT_SVC_LOCATE_HANDLE_BUFFER);
    let open_protocol: OpenProtocolFn =
        read_boot_svc_fn(gbs, BOOT_SVC_OPEN_PROTOCOL);
    let free_pool: FreePoolFn =
        read_boot_svc_fn(gbs, BOOT_SVC_FREE_POOL);

    let mut handle_count: UINTN = 0;
    let mut handle_buffer: *mut EFI_HANDLE = core::ptr::null_mut();
    let status = unsafe {
        locate_handle_buffer(
            2,
            &SNP_GUID as *const EFI_GUID,
            core::ptr::null_mut(),
            &mut handle_count,
            &mut handle_buffer,
        )
    };

    if status != EFI_SUCCESS || handle_count == 0 {
        w16(con_out, "No network adapters found.\r\n");
        return;
    }

    for i in 0..handle_count.min(1) {
        let handle = unsafe { *handle_buffer.add(i as usize) };

        let mut snp_ptr: *mut c_void = core::ptr::null_mut();
        let st = unsafe {
            open_protocol(
                handle,
                &SNP_GUID as *const EFI_GUID,
                &mut snp_ptr,
                image_handle,
                core::ptr::null_mut(),
                EFI_OPEN_PROTOCOL_GET_PROTOCOL,
            )
        };

        if st != EFI_SUCCESS {
            w16(con_out, "Cannot open SNP protocol.\r\n");
            continue;
        }

        let snp = unsafe { &*(snp_ptr as *const EFI_SIMPLE_NETWORK_PROTOCOL) };

        // Start and initialize the NIC
        let rst = unsafe { (snp.start)(snp as *const _ as *mut _) };
        if rst != EFI_SUCCESS && rst != EFI_ALREADY_STARTED && rst != (EFI_ALREADY_STARTED | (1 << 63)) {
            continue;
        }

        let rst = unsafe { (snp.initialize)(snp as *const _ as *mut _, 0, 0) };
        if rst != EFI_SUCCESS {
            // Some firmware (ARM64 virtio) doesn't support Initialize;
            // the NIC is still usable.
        }

        let mode = unsafe { &*snp.mode };
        let hw_addr_size = mode.hw_address_size as usize;
        if hw_addr_size < 6 { continue; }

        // Read MAC from current_address
        let mac: [u8; 6] = [
            mode.current_address.addr[0],
            mode.current_address.addr[1],
            mode.current_address.addr[2],
            mode.current_address.addr[3],
            mode.current_address.addr[4],
            mode.current_address.addr[5],
        ];

        w16(con_out, "Network adapter:\r\n");
        w16(con_out, "  MAC: ");
        print_mac(con_out, &mac);
        w16(con_out, "\r\n");

        // Run DHCP
        w16(con_out, "  DHCP: ");
        match dhcp_run(con_out, snp, &mac) {
            Some(cfg) => {
                w16(con_out, "OK\r\n");
                w16(con_out, "  IP: ");
                print_ip(con_out, &cfg.yiaddr);
                w16(con_out, "\r\n  Subnet: ");
                print_ip(con_out, &cfg.subnet);
                w16(con_out, "\r\n  Gateway: ");
                if cfg.gateway == [0,0,0,0] { w16(con_out, "(none)"); }
                else { print_ip(con_out, &cfg.gateway); }
                w16(con_out, "\r\n");
            }
            None => {
                w16(con_out, "FAILED\r\n");
            }
        }
    }

    unsafe { free_pool(handle_buffer as *mut c_void); }
}

fn dhcp_run(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    snp: &EFI_SIMPLE_NETWORK_PROTOCOL,
    mac: &[u8; 6],
) -> Option<DhcpConfig> {
    let xid: u32 = 0x12345678;

    // Wait for link
    for _ in 0..50000 {
        let mode = unsafe { &*snp.mode };
        if mode.media_present != 0 { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }

    // ARM64 virtio SNP only supports one transmit per session
    // (Initialize is unsupported, driver has no buffer recycling).
    // Send DISCOVER, then listen for OFFER.
    w16(con_out, "Sending DHCPDISCOVER...");
    let discover = make_dhcp_discover(xid, mac);
    if !send_udp_dhcp(snp, mac, &discover, 300) {
        w16(con_out, "send failed\r\n");
        return None;
    }
    w16(con_out, "sent, waiting for OFFER...\r\n");

    for _ in 0..500000 {
        if let Some(cfg) = try_receive(snp, xid, mac) {
            return Some(cfg);
        }
        for _ in 0..20 { core::hint::spin_loop(); }
    }

    w16(con_out, "timeout\r\n");
    None
}

fn send_udp_dhcp(
    snp: &EFI_SIMPLE_NETWORK_PROTOCOL,
    mac: &[u8; 6],
    dhcp_payload: &[u8; 300],
    dhcp_len: usize,
) -> bool {
    let mut frame = [0u8; 1514];
    let frame_len = build_eth_ip_udp_dhcp(mac, dhcp_payload, dhcp_len, &mut frame);

    let st = unsafe {
        (snp.transmit)(
            snp as *const _ as *mut _,
            0,
            frame_len as UINTN,
            frame.as_mut_ptr() as *mut c_void,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    st == EFI_SUCCESS
}

fn try_receive(
    snp: &EFI_SIMPLE_NETWORK_PROTOCOL,
    xid: u32,
    mac: &[u8; 6],
) -> Option<DhcpConfig> {
    let mut frame = [0u8; 1514];
    let mut header_size: UINTN = 0;
    let mut buffer_size: UINTN = 1514;
    let mut src_addr = EFI_MAC_ADDRESS { addr: [0u8; 32] };
    let mut dst_addr = EFI_MAC_ADDRESS { addr: [0u8; 32] };
    let mut protocol: u16 = 0;

    let st = unsafe {
        (snp.receive)(
            snp as *const _ as *mut _,
            &mut header_size,
            &mut buffer_size,
            frame.as_mut_ptr() as *mut c_void,
            &mut src_addr,
            &mut dst_addr,
            &mut protocol,
        )
    };

    if st != EFI_SUCCESS {
        return None;
    }

    parse_dhcp_response(&frame, buffer_size as usize, xid, mac)
}

pub fn scan_network_devices(
    image_handle: EFI_HANDLE,
    system_table: &EFI_SYSTEM_TABLE,
) {
    let con_out = unsafe { &*system_table.con_out };
    let gbs = system_table.boot_services;

    w16(con_out, "Scanning for network adapters...\r\n\r\n");
    print_network_info(con_out, gbs, image_handle);
}
