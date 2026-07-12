use core::ffi::c_void;

use crate::efi::*;

type LocateHandleBufferFn = unsafe extern "efiapi" fn(
    search_type: UINTN,
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

pub fn w16(con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL, s: &str) {
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
    pkt[0] = 1;
    pkt[1] = 1;
    pkt[2] = 6;
    pkt[4..8].copy_from_slice(&xid.to_be_bytes());
    pkt[10] = 0x80;
    pkt[12..16].fill(0);
    pkt[16..20].fill(0);
    pkt[20..24].fill(0);
    pkt[24..28].fill(0);
    pkt[28..34].copy_from_slice(mac);
    pkt[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);
    let mut off = 240;
    pkt[off] = 53;  pkt[off + 1] = 1;  pkt[off + 2] = 1;
    off += 3;
    pkt[off] = 55;  pkt[off + 1] = 3;  pkt[off + 2] = 1;
    pkt[off + 3] = 3;  pkt[off + 4] = 6;
    off += 5;
    pkt[off] = 255;
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
    buf[0..6].fill(0xFF);
    buf[6..12].copy_from_slice(mac);
    buf[12] = 0x08; buf[13] = 0x00;

    let ip_off = 14usize;
    let ip_total_len = 20 + 8 + dhcp_len;

    buf[ip_off] = 0x45;
    buf[ip_off + 1] = 0x00;
    buf[ip_off + 2..ip_off + 4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    buf[ip_off + 4..ip_off + 6].copy_from_slice(&[0x00, 0x00]);
    buf[ip_off + 6..ip_off + 8].copy_from_slice(&[0x00, 0x00]);
    buf[ip_off + 8] = 64;
    buf[ip_off + 9] = 17;
    buf[ip_off + 10..ip_off + 12].copy_from_slice(&[0x00, 0x00]);
    buf[ip_off + 12..ip_off + 16].fill(0x00);
    buf[ip_off + 16..ip_off + 20].fill(0xFF);

    let cksum = ip_checksum(&buf[ip_off..ip_off + 20]);
    buf[ip_off + 10..ip_off + 12].copy_from_slice(&cksum.to_be_bytes());

    let udp_off = ip_off + 20;
    let udp_len = 8 + dhcp_len;
    buf[udp_off..udp_off + 2].copy_from_slice(&[0x00, 0x44]);
    buf[udp_off + 2..udp_off + 4].copy_from_slice(&[0x00, 0x43]);
    buf[udp_off + 4..udp_off + 6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    buf[udp_off + 6..udp_off + 8].copy_from_slice(&[0x00, 0x00]);

    let dhcp_off = udp_off + 8;
    buf[dhcp_off..dhcp_off + dhcp_len].copy_from_slice(&dhcp_payload[..dhcp_len]);
    dhcp_off + dhcp_len
}

fn parse_dhcp_response(buf: &[u8], len: usize, xid: u32, mac: &[u8; 6]) -> Option<DhcpConfig> {
    if len < 282 { return None; }
    if buf[12] != 0x08 || buf[13] != 0x00 { return None; }
    let ip_off = 14;
    let ip_hdr_len = (buf[ip_off] & 0x0F) as usize * 4;
    if ip_hdr_len < 20 { return None; }
    if buf[ip_off + 9] != 17 { return None; }
    let udp_off = ip_off + ip_hdr_len;
    let dhcp_off = udp_off + 8;
    let dhcp_len = len - dhcp_off;
    if dhcp_len < 240 { return None; }
    if dhcp_off + 4 > len { return None; }
    if buf[dhcp_off + 236] != 0x63 || buf[dhcp_off + 237] != 0x82
        || buf[dhcp_off + 238] != 0x53 || buf[dhcp_off + 239] != 0x63 {
        return None;
    }
    let pkt_xid = u32::from_be_bytes([
        buf[dhcp_off + 4], buf[dhcp_off + 5],
        buf[dhcp_off + 6], buf[dhcp_off + 7],
    ]);
    if pkt_xid != xid { return None; }
    for i in 0..6 {
        if buf[dhcp_off + 28 + i] != mac[i] { return None; }
    }
    let yiaddr: [u8; 4] = [
        buf[dhcp_off + 16], buf[dhcp_off + 17],
        buf[dhcp_off + 18], buf[dhcp_off + 19],
    ];
    let mut subnet = [255u8; 4];
    let mut gateway = [0u8; 4];
    let mut dhcp_msg_type = 0u8;
    let mut off = dhcp_off + 240;
    while off + 1 < len {
        let opt_type = buf[off];
        if opt_type == 255 { break; }
        let opt_len = buf[off + 1] as usize;
        if off + 2 + opt_len > len { break; }
        if opt_type == 53 && opt_len == 1 {
            dhcp_msg_type = buf[off + 2];
        } else if opt_type == 1 && opt_len == 4 {
            subnet.copy_from_slice(&buf[off + 2..off + 6]);
        } else if opt_type == 3 && opt_len >= 4 {
            gateway.copy_from_slice(&buf[off + 2..off + 6]);
        }
        off += 2 + opt_len;
    }
    if dhcp_msg_type != 2 && dhcp_msg_type != 5 { return None; }
    Some(DhcpConfig { yiaddr, subnet, gateway })
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

// ─── e1000 direct MMIO driver ───

const REG_CTRL: u64 = 0x0000;
const REG_STATUS: u64 = 0x0008;
const REG_RCTL: u64 = 0x0100;
const REG_TCTL: u64 = 0x0400;
const REG_RDBAL: u64 = 0x2800;
const REG_RDBAH: u64 = 0x2804;
const REG_RDLEN: u64 = 0x2808;
const REG_RDH: u64 = 0x2810;
const REG_RDT: u64 = 0x2818;
const REG_TDBAL: u64 = 0x3800;
const REG_TDBAH: u64 = 0x3804;
const REG_TDLEN: u64 = 0x3808;
const REG_TDH: u64 = 0x3810;
const REG_TDT: u64 = 0x3818;
const REG_RA: u64 = 0x5400;
const REG_MTA: u64 = 0x5200;

const CTRL_RST: u32 = 0x0400_0000;
const CTRL_SLU: u32 = 0x0000_0040;
const CTRL_FD: u32 = 0x0000_0001;
const STATUS_LU: u32 = 0x0000_0002;

const RCTL_EN: u32 = 0x0000_0002;
const RCTL_UPE: u32 = 0x0000_0008;
const RCTL_MPE: u32 = 0x0000_0010;
const RCTL_BAM: u32 = 0x0000_8000;
const RCTL_BSIZE_SHIFT: u32 = 16;
const RCTL_SECRC: u32 = 0x0800_0000;

const TCTL_EN: u32 = 0x0000_0002;
const TCTL_PSP: u32 = 0x0000_0008;
const TCTL_CT_SHIFT: u32 = 4;
const TCTL_COLD_SHIFT: u32 = 12;

const CMD_EOP: u8 = 0x01;
const CMD_IFCS: u8 = 0x02;
const CMD_RS: u8 = 0x08;

const RX_STATUS_DD: u8 = 0x01;
const TX_STATUS_DD: u8 = 0x01;

const RX_BUFFER_SIZE: usize = 2048;
const NUM_RX_DESC: usize = 8;
const NUM_TX_DESC: usize = 8;

#[repr(C, packed)]
#[derive(Copy, Clone)]
struct RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C, packed)]
#[derive(Copy, Clone)]
struct TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

#[repr(align(16))]
struct RxDescs([RxDesc; NUM_RX_DESC]);

#[repr(align(16))]
struct TxDescs([TxDesc; NUM_TX_DESC]);

static mut RX_DESCS: RxDescs = RxDescs([RxDesc { addr: 0, length: 0, checksum: 0, status: 0, errors: 0, special: 0 }; NUM_RX_DESC]);
static mut TX_DESCS: TxDescs = TxDescs([TxDesc { addr: 0, length: 0, cso: 0, cmd: 0, status: 0, css: 0, special: 0 }; NUM_TX_DESC]);
static mut RX_BUF: [u8; RX_BUFFER_SIZE] = [0u8; RX_BUFFER_SIZE];
static mut TX_BUF: [u8; 2048] = [0u8; 2048];

// ─── Direct MMIO e1000 driver (no UEFI protocols) ───

struct DirectMmioE1000 {
    base: *mut u32,
}

impl DirectMmioE1000 {
    fn new(bar0: u64) -> Self {
        Self { base: bar0 as *mut u32 }
    }

    fn mmio_read32(&self, reg: u64) -> u32 {
        unsafe { core::ptr::read_volatile(self.base.add((reg / 4) as usize)) }
    }

    fn mmio_write32(&self, reg: u64, val: u32) {
        unsafe { core::ptr::write_volatile(self.base.add((reg / 4) as usize), val) }
    }

    fn read_mac(&self) -> [u8; 6] {
        let low = self.mmio_read32(REG_RA);
        let high = self.mmio_read32(REG_RA + 4);
        [
            low as u8,
            (low >> 8) as u8,
            (low >> 16) as u8,
            (low >> 24) as u8,
            high as u8,
            (high >> 8) as u8,
        ]
    }

    fn set_mac(&self, mac: &[u8; 6]) {
        let low = mac[0] as u32 | (mac[1] as u32) << 8 | (mac[2] as u32) << 16 | (mac[3] as u32) << 24;
        let high = mac[4] as u32 | (mac[5] as u32) << 8;
        self.mmio_write32(REG_RA, low);
        self.mmio_write32(REG_RA + 4, high | 0x8000_0001);
    }

    fn clear_multicast(&self) {
        for i in 0..128 {
            self.mmio_write32(REG_MTA + (i as u64) * 4, 0);
        }
    }

    fn init(&self) -> bool {
        self.mmio_write32(REG_CTRL, self.mmio_read32(REG_CTRL) | CTRL_RST);
        for _ in 0..100000 {
            if self.mmio_read32(REG_CTRL) & CTRL_RST == 0 { break; }
            core::hint::spin_loop();
        }
        for _ in 0..1000000 {
            if self.mmio_read32(REG_STATUS) & STATUS_LU != 0 { break; }
            core::hint::spin_loop();
        }

        let mac = self.read_mac();
        if mac == [0u8; 6] || mac == [0xFFu8; 6] {
            return false;
        }

        self.set_mac(&mac);
        self.clear_multicast();

        unsafe {
            let rx_buf_addr = core::ptr::addr_of!(RX_BUF) as *const u8 as u64;
            for i in 0..NUM_RX_DESC {
                RX_DESCS.0[i].addr = rx_buf_addr;
            }
            TX_DESCS.0[0].addr = core::ptr::addr_of!(TX_BUF) as *const u8 as u64;
        }

        let rx_descs_addr = core::ptr::addr_of!(RX_DESCS) as u64;
        let tx_descs_addr = core::ptr::addr_of!(TX_DESCS) as u64;

        self.mmio_write32(REG_RDBAL, rx_descs_addr as u32);
        self.mmio_write32(REG_RDBAH, 0);
        self.mmio_write32(REG_RDLEN, (NUM_RX_DESC * 16) as u32);
        self.mmio_write32(REG_RDH, 0);
        self.mmio_write32(REG_RDT, (NUM_RX_DESC - 1) as u32);

        self.mmio_write32(REG_TDBAL, tx_descs_addr as u32);
        self.mmio_write32(REG_TDBAH, 0);
        self.mmio_write32(REG_TDLEN, (NUM_TX_DESC * 16) as u32);
        self.mmio_write32(REG_TDH, 0);
        self.mmio_write32(REG_TDT, 0);

        let rctl = RCTL_EN | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_SECRC | (0 << RCTL_BSIZE_SHIFT);
        self.mmio_write32(REG_RCTL, rctl);

        let tctl = TCTL_EN | TCTL_PSP | (0x0F << TCTL_CT_SHIFT) | (0x3F << TCTL_COLD_SHIFT);
        self.mmio_write32(REG_TCTL, tctl);

        self.mmio_write32(REG_CTRL, self.mmio_read32(REG_CTRL) | CTRL_SLU | CTRL_FD);

        for _ in 0..1000000 {
            if self.mmio_read32(REG_STATUS) & STATUS_LU != 0 { break; }
            core::hint::spin_loop();
        }

        true
    }

    fn send(&self, data: &[u8]) -> bool {
        if data.len() > 2048 { return false; }
        unsafe {
            let buf = core::ptr::addr_of_mut!(TX_BUF) as *mut u8;
            for i in 0..data.len() {
                *buf.add(i) = data[i];
            }
            TX_DESCS.0[0].length = data.len() as u16;
            TX_DESCS.0[0].cmd = CMD_EOP | CMD_IFCS | CMD_RS;
            TX_DESCS.0[0].status = 0;
        }
        let old_tdt = self.mmio_read32(REG_TDT);
        self.mmio_write32(REG_TDT, old_tdt.wrapping_add(1));
        for _ in 0..2000000 {
            unsafe {
                if TX_DESCS.0[0].status & TX_STATUS_DD != 0 { break; }
            }
            core::hint::spin_loop();
        }
        unsafe { TX_DESCS.0[0].status & TX_STATUS_DD != 0 }
    }

    fn receive_into(&self, buf: &mut [u8; 1514]) -> Option<usize> {
        for idx in 0..NUM_RX_DESC {
            unsafe {
                if RX_DESCS.0[idx].status & RX_STATUS_DD != 0 {
                    let len = RX_DESCS.0[idx].length as usize;
                    let copy_len = if len > 1514 { 1514 } else { len };
                    core::ptr::copy_nonoverlapping(
                        core::ptr::addr_of!(RX_BUF) as *const u8,
                        buf.as_mut_ptr(),
                        copy_len,
                    );
                    RX_DESCS.0[idx].status = 0;
                    self.mmio_write32(REG_RDT, idx as u32);
                    return Some(copy_len);
                }
            }
        }
        None
    }

    fn dhcp_run(&self) -> Option<DhcpConfig> {
        let mac = self.read_mac();
        let xid = 0x12345678;
        let mut frame = [0u8; 1514];
        let dhcp_payload = make_dhcp_discover(xid, &mac);
        let frame_len = build_eth_ip_udp_dhcp(&mac, &dhcp_payload, 300, &mut frame);
        if !self.send(&frame[..frame_len]) {
            return None;
        }

        for _ in 0..200_000_000 {
            if let Some(len) = self.receive_into(&mut frame) {
                if let Some(cfg) = parse_dhcp_response(&frame, len, xid, &mac) {
                    return Some(cfg);
                }
            }
            core::hint::spin_loop();
        }
        None
    }
}

fn scan_pci_direct(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
) -> Option<DhcpConfig> {
    w16(con_out, "Scanning PCI buses via I/O ports...\r\n");
    for bus in 0..=255u8 {
        for dev in 0..32u8 {
            for func in 0..8u8 {
                let vendor_dev = pci_read_config32(bus, dev, func, 0);
                if vendor_dev == 0xFFFFFFFF {
                    if func == 0 { break; }
                    continue;
                }
                let vendor = vendor_dev as u16;
                let device = (vendor_dev >> 16) as u16;
                if vendor == 0x8086 && device == 0x100E {
                    w16(con_out, "Found e1000 at ");
                    put_dec(con_out, bus as u64);
                    w16(con_out, ":");
                    put_dec(con_out, dev as u64);
                    w16(con_out, ".");
                    put_dec(con_out, func as u64);
                    w16(con_out, "\r\n");

                    let bar0 = pci_read_config32(bus, dev, func, 0x10) & !0xF;
                    if bar0 == 0 {
                        w16(con_out, "  BAR0 is 0, skipping\r\n");
                        continue;
                    }
                    w16(con_out, "  BAR0=0x");
                    put_dec(con_out, bar0 as u64);
                    w16(con_out, "\r\n");

                    let e1000 = DirectMmioE1000::new(bar0 as u64);
                    if !e1000.init() {
                        w16(con_out, "  e1000 init failed\r\n");
                        continue;
                    }
                    let mac = e1000.read_mac();
                    w16(con_out, "  MAC: ");
                    for i in 0..6 {
                        put_dec(con_out, mac[i] as u64);
                        if i < 5 { w16(con_out, ":"); }
                    }
                    w16(con_out, "\r\n  DHCP: ");
                    match e1000.dhcp_run() {
                        Some(cfg) => {
                            w16(con_out, "OK\r\n");
                            w16(con_out, "  IP: ");
                            print_ip(con_out, &cfg.yiaddr);
                            w16(con_out, "\r\n  Subnet: ");
                            print_ip(con_out, &cfg.subnet);
                            w16(con_out, "\r\n  Gateway: ");
                            if cfg.gateway == [0, 0, 0, 0] { w16(con_out, "(none)"); }
                            else { print_ip(con_out, &cfg.gateway); }
                            w16(con_out, "\r\n");
                            return Some(cfg);
                        }
                        None => {
                            w16(con_out, "FAILED\r\n");
                        }
                    }
                }
            }
        }
    }
    w16(con_out, "No e1000 found\r\n");
    None
}

// ─── PCI scanning and network via direct e1000 ───

fn scan_e1000_devices(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    gbs: *mut c_void,
    image_handle: EFI_HANDLE,
) -> Option<DhcpConfig> {
    let _open_protocol: OpenProtocolFn = read_boot_svc_fn(gbs, BOOT_SVC_OPEN_PROTOCOL);

    // Try 1: Get device path from image handle to find PCI location
    w16(con_out, "Trying Device Path on image handle...\r\n");
    if let Some(cfg) = try_device_path(con_out, gbs, image_handle) {
        return Some(cfg);
    }

    // Try 2: Enumerate all handles with PCI IO protocol (works in normal UEFI post-DXE)
    let locate_handle_buffer: LocateHandleBufferFn = read_boot_svc_fn(gbs, BOOT_SVC_LOCATE_HANDLE_BUFFER);
    let open_protocol: OpenProtocolFn = read_boot_svc_fn(gbs, BOOT_SVC_OPEN_PROTOCOL);
    let free_pool: FreePoolFn = read_boot_svc_fn(gbs, BOOT_SVC_FREE_POOL);

    w16(con_out, "Trying PCI IO protocol handles...\r\n");
    let mut handle_count: UINTN = 0;
    let mut handle_buffer: *mut EFI_HANDLE = core::ptr::null_mut();
    let st = unsafe {
        locate_handle_buffer(
            2, // ByProtocol
            &PCI_IO_GUID as *const EFI_GUID,
            core::ptr::null_mut(),
            &mut handle_count,
            &mut handle_buffer,
        )
    };

    if st != EFI_SUCCESS {
        w16(con_out, "LocateHandleBuffer(PCI_IO) failed: status=");
        put_dec(con_out, st as u64);
        w16(con_out, "\r\n");
    } else if handle_count == 0 {
        w16(con_out, "No PCI IO handles found\r\n");
    } else {
        w16(con_out, "Found ");
        put_dec(con_out, handle_count as u64);
        w16(con_out, " PCI IO handles\r\n");

        for i in 0..handle_count {
            let handle = unsafe { *handle_buffer.add(i as usize) };
            if let Some(cfg) = scan_pci_io_handle(con_out, handle, open_protocol, image_handle) {
                unsafe { free_pool(handle_buffer as *mut c_void); }
                return Some(cfg);
            }
        }
    }
    unsafe { free_pool(handle_buffer as *mut c_void); }

    // Try 3: Loaded Image protocol
    w16(con_out, "Trying Loaded Image protocol...\r\n");
    if let Some(cfg) = try_loaded_image_path(con_out, gbs, image_handle) {
        return Some(cfg);
    }

    // Try 4: Direct PCI bus scan via I/O ports (works in all phases, no protocols needed)
    w16(con_out, "Trying direct PCI scan via I/O ports...\r\n");
    if let Some(cfg) = scan_pci_direct(con_out) {
        return Some(cfg);
    }

    None
}

fn e1000_init_and_dhcp(con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL, bar0: u64) -> Option<DhcpConfig> {
    w16(con_out, "e1000 init at BAR0=0x");
    put_dec(con_out, bar0);
    w16(con_out, "\r\n");

    let e1000 = DirectMmioE1000::new(bar0);
    if !e1000.init() {
        w16(con_out, "e1000 init failed\r\n");
        return None;
    }

    let mac = e1000.read_mac();
    w16(con_out, "MAC: ");
    for b in &mac {
        put_dec(con_out, *b as u64);
        w16(con_out, if *b as u64 == mac[5] as u64 { "" } else { ":" });
    }
    w16(con_out, "\r\n");

    let config = e1000.dhcp_run()?;

    let ip_bytes = config.yiaddr;
    w16(con_out, "DHCP: IP=");
    put_dec(con_out, ip_bytes[0] as u64);
    w16(con_out, ".");
    put_dec(con_out, ip_bytes[1] as u64);
    w16(con_out, ".");
    put_dec(con_out, ip_bytes[2] as u64);
    w16(con_out, ".");
    put_dec(con_out, ip_bytes[3] as u64);
    w16(con_out, " Gateway=");
    let gw_bytes = config.gateway;
    put_dec(con_out, gw_bytes[0] as u64);
    w16(con_out, ".");
    put_dec(con_out, gw_bytes[1] as u64);
    w16(con_out, ".");
    put_dec(con_out, gw_bytes[2] as u64);
    w16(con_out, ".");
    put_dec(con_out, gw_bytes[3] as u64);
    w16(con_out, "\r\n");

    Some(config)
}

fn try_loaded_image_path(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    gbs: *mut c_void,
    image_handle: EFI_HANDLE,
) -> Option<DhcpConfig> {
    let open_protocol: OpenProtocolFn = read_boot_svc_fn(gbs, BOOT_SVC_OPEN_PROTOCOL);

    w16(con_out, "Trying Loaded Image protocol...\r\n");
    let mut loaded_image_ptr: *mut c_void = core::ptr::null_mut();
    let st = unsafe {
        open_protocol(
            image_handle,
            &LOADED_IMAGE_GUID as *const EFI_GUID,
            &mut loaded_image_ptr,
            image_handle,
            core::ptr::null_mut(),
            EFI_OPEN_PROTOCOL_GET_PROTOCOL,
        )
    };
    if st != EFI_SUCCESS {
        w16(con_out, "Loaded Image protocol failed: status=");
        put_dec(con_out, st as u64);
        w16(con_out, "\r\n");
        return None;
    }
    let loaded_image = unsafe { &*(loaded_image_ptr as *const EFI_LOADED_IMAGE_PROTOCOL) };
    let device_handle = loaded_image.device_handle;
    let _file_path = loaded_image.file_path;

    w16(con_out, "Loaded Image: device_handle=");
    put_dec(con_out, device_handle as u64);
    w16(con_out, "\r\n");

    // Try to open PCI IO on device handle
    let open_protocol_fn: OpenProtocolFn = read_boot_svc_fn(gbs, BOOT_SVC_OPEN_PROTOCOL);
    let mut pci_io_ptr: *mut c_void = core::ptr::null_mut();
    let st = unsafe {
        open_protocol_fn(
            device_handle,
            &PCI_IO_GUID as *const EFI_GUID,
            &mut pci_io_ptr,
            image_handle,
            core::ptr::null_mut(),
            EFI_OPEN_PROTOCOL_GET_PROTOCOL,
        )
    };
    if st != EFI_SUCCESS {
        w16(con_out, "PCI IO on device handle failed: status=");
        put_dec(con_out, st as u64);
        w16(con_out, "\r\n");
        return None;
    }

    w16(con_out, "PCI IO opened on device handle!\r\n");
    let pci_io = pci_io_ptr as *mut EFI_PCI_IO_PROTOCOL;
    let pci_access = unsafe { &(*pci_io).pci };
    let mut vendor_dev: u32 = 0;
    let st = unsafe {
        (pci_access.read)(
            pci_io,
            EFI_PCI_IO_PROTOCOL_WIDTH::Uint32,
            0x00,
            &mut vendor_dev as *mut _ as *mut c_void,
        )
    };
    if st != EFI_SUCCESS {
        w16(con_out, "PCI config read failed\r\n");
        return None;
    }
    let vendor = vendor_dev as u16;
    let device = (vendor_dev >> 16) as u16;
    w16(con_out, "Device: vendor=0x");
    put_dec(con_out, vendor as u64);
    w16(con_out, " device=0x");
    put_dec(con_out, device as u64);
    w16(con_out, "\r\n");

    if vendor == 0x8086 && device == 0x100E {
        let mut bar0: u32 = 0;
        let st = unsafe {
            (pci_access.read)(
                pci_io,
                EFI_PCI_IO_PROTOCOL_WIDTH::Uint32,
                0x10,
                &mut bar0 as *mut _ as *mut c_void,
            )
        };
        if st == EFI_SUCCESS {
            let bar0 = bar0 & !0xF;
            if bar0 != 0 {
                w16(con_out, "Found e1000 at BAR0=0x");
                put_dec(con_out, bar0 as u64);
                w16(con_out, "\r\n");
                return e1000_init_and_dhcp(con_out, bar0 as u64);
            }
        }
    }
None
}

#[cfg(target_arch = "x86_64")]
fn pci_read_config32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let addr: u32 = 0x8000_0000
        | (bus as u32) << 16
        | (dev as u32) << 11
        | (func as u32) << 8
        | (offset as u32 & 0xFC);
    let cfg_port: u16 = 0xCF8;
    let data_port: u16 = 0xCFC;
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") cfg_port,
            in("eax") addr,
        );
        let val: u32;
        core::arch::asm!(
            "in eax, dx",
            in("dx") data_port,
            out("eax") val,
        );
        val
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn pci_read_config32(bus: u8, dev: u8, func: u8, offset: u8) -> u32 {
    let ecam_base: u64 = 0x4010_0000_0000;
    let addr = ecam_base | ((bus as u64) << 20) | ((dev as u64) << 15) | ((func as u64) << 12) | (offset as u64);
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn try_device_path(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    gbs: *mut c_void,
    image_handle: EFI_HANDLE,
) -> Option<DhcpConfig> {
    let open_protocol: OpenProtocolFn = read_boot_svc_fn(gbs, BOOT_SVC_OPEN_PROTOCOL);

    w16(con_out, "Trying Device Path protocol...\r\n");
    let mut dp_ptr: *mut c_void = core::ptr::null_mut();
    let st = unsafe {
        open_protocol(
            image_handle,
            &DEVICE_PATH_GUID as *const EFI_GUID,
            &mut dp_ptr,
            image_handle,
            core::ptr::null_mut(),
            EFI_OPEN_PROTOCOL_GET_PROTOCOL,
        )
    };
    if st != EFI_SUCCESS {
        w16(con_out, "Device Path protocol failed: status=");
        put_dec(con_out, st as u64);
        w16(con_out, "\r\n");
        return None;
    }
    w16(con_out, "Device Path protocol opened\r\n");

    #[repr(C, packed)]
    struct PciDevicePathNode {
        type_: u8,
        sub_type: u8,
        length: u16,
        bus: u8,
        dev_func: u8,
    }

    let mut offset: usize = 0;
    let mut pci_bus: u8 = 0;
    let mut pci_dev: u8 = 0;
    let mut pci_func: u8 = 0;
    let mut found_pci = false;

    // Parse device path nodes
    loop {
        let node = unsafe { &*((dp_ptr as *const u8).add(offset) as *const EFI_DEVICE_PATH_PROTOCOL) };
        let node_len = node.length as usize;
        if node.type_ == 0x01 && node.sub_type == 0x01 && node_len >= 6 {
            // PCI device path node (device/function packed into one byte)
            let pci_node = unsafe { &*((dp_ptr as *const u8).add(offset) as *const PciDevicePathNode) };
            pci_bus = pci_node.bus;
            pci_dev = (pci_node.dev_func >> 3) & 0x1F;
            pci_func = pci_node.dev_func & 0x07;
            found_pci = true;
            w16(con_out, "Found PCI node: bus=");
            put_dec(con_out, pci_bus as u64);
            w16(con_out, " dev=");
            put_dec(con_out, pci_dev as u64);
            w16(con_out, " func=");
            put_dec(con_out, pci_func as u64);
            w16(con_out, "\r\n");
            break;
        }
        if node.type_ == 0x7F && node.sub_type == 0xFF {
            break;
        }
        offset += node_len;
        if node_len == 0 { break; }
    }

    if !found_pci {
        w16(con_out, "No PCI node in device path\r\n");
        return None;
    }

    // Use direct PCI config space access (via I/O ports on x86, ECAM on ARM64)
    {
        let vendor_dev = pci_read_config32(pci_bus, pci_dev, pci_func, 0x00);
        let vendor = vendor_dev as u16;
        let device = (vendor_dev >> 16) as u16;
        w16(con_out, "Vendor=0x");
        put_dec(con_out, vendor as u64);
        w16(con_out, " Device=0x");
        put_dec(con_out, device as u64);
        w16(con_out, "\r\n");

        if vendor == 0x8086 && device == 0x100E {
            let bar0_raw = pci_read_config32(pci_bus, pci_dev, pci_func, 0x10);
            let bar0 = (bar0_raw & !0xF) as u64;
            if bar0 != 0 {
                w16(con_out, "BAR0=0x");
                put_dec(con_out, bar0);
                w16(con_out, "\r\n");
                return e1000_init_and_dhcp(con_out, bar0);
            }
        }
    }

    None
}

fn scan_pci_io_handle(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    handle: EFI_HANDLE,
    open_protocol: OpenProtocolFn,
    image_handle: EFI_HANDLE,
) -> Option<DhcpConfig> {
    // Try to get the device path to find PCI location
    let mut dp_ptr: *mut c_void = core::ptr::null_mut();
    let dp_st = unsafe {
        open_protocol(
            handle,
            &DEVICE_PATH_GUID as *const EFI_GUID,
            &mut dp_ptr,
            image_handle,
            core::ptr::null_mut(),
            EFI_OPEN_PROTOCOL_GET_PROTOCOL,
        )
    };
    if dp_st != EFI_SUCCESS {
        return None;
    }

    #[repr(C, packed)]
    struct PciNode {
        type_: u8,
        sub_type: u8,
        length: u16,
        bus: u8,
        dev_func: u8,
    }

    let mut offset: usize = 0;
    let mut pci_bus: u8 = 0xff;
    let mut pci_dev: u8 = 0xff;
    let mut pci_func: u8 = 0xff;

    loop {
        let node = unsafe { &*((dp_ptr as *const u8).add(offset) as *const PciNode) };
        let node_len = node.length as usize;
        if node.type_ == 0x01 && node.sub_type == 0x01 && node_len >= 6 {
            pci_bus = node.bus;
            pci_dev = (node.dev_func >> 3) & 0x1F;
            pci_func = node.dev_func & 0x07;
            break;
        }
        if node.type_ == 0x7F && node.sub_type == 0xFF {
            break;
        }
        offset += node_len;
        if node_len == 0 { break; }
    }

    if pci_bus == 0xff {
        return None;
    }

    let vendor_dev = pci_read_config32(pci_bus, pci_dev, pci_func, 0x00);
    let vendor = vendor_dev as u16;
    let device = (vendor_dev >> 16) as u16;
    w16(con_out, "PCI IO handle: bus=");
    put_dec(con_out, pci_bus as u64);
    w16(con_out, " dev=");
    put_dec(con_out, pci_dev as u64);
    w16(con_out, " func=");
    put_dec(con_out, pci_func as u64);
    w16(con_out, " vendor=0x");
    put_dec(con_out, vendor as u64);
    w16(con_out, " device=0x");
    put_dec(con_out, device as u64);
    w16(con_out, "\r\n");
    if vendor != 0x8086 || device != 0x100E {
        return None;
    }

    let bar0_raw = pci_read_config32(pci_bus, pci_dev, pci_func, 0x10);
    let bar0 = (bar0_raw & !0xF) as u64;
    if bar0 == 0 {
        return None;
    }

    w16(con_out, "Found e1000 via PCI IO: BAR0=0x");
    put_dec(con_out, bar0);
    w16(con_out, "\r\n");
    e1000_init_and_dhcp(con_out, bar0)
}

// ─── SNP-based network scan (original) ───

fn dhcp_run(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    snp: &EFI_SIMPLE_NETWORK_PROTOCOL,
    mac: &[u8; 6],
) -> Option<DhcpConfig> {
    let xid: u32 = 0x12345678;

    for _ in 0..50000 {
        let mode = unsafe { &*snp.mode };
        if mode.media_present != 0 { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }

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

// ─── Public API ───

pub fn scan_network_devices(
    image_handle: EFI_HANDLE,
    system_table: &EFI_SYSTEM_TABLE,
) {
    let con_out = unsafe { &*system_table.con_out };
    let gbs = system_table.boot_services;

    w16(con_out, "Scanning for network adapters...\r\n\r\n");

    let locate_handle_buffer: LocateHandleBufferFn = read_boot_svc_fn(gbs, BOOT_SVC_LOCATE_HANDLE_BUFFER);
    let open_protocol: OpenProtocolFn = read_boot_svc_fn(gbs, BOOT_SVC_OPEN_PROTOCOL);
    let free_pool: FreePoolFn = read_boot_svc_fn(gbs, BOOT_SVC_FREE_POOL);

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

    let mut snp_handled = false;

    if status == EFI_SUCCESS && handle_count > 0 {
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
            let rst = unsafe { (snp.start)(snp as *const _ as *mut _) };
            if rst != EFI_SUCCESS && rst != EFI_ALREADY_STARTED && rst != (EFI_ALREADY_STARTED | (1 << 63)) {
                continue;
            }
            let rst = unsafe { (snp.initialize)(snp as *const _ as *mut _, 0, 0) };
            if rst != EFI_SUCCESS {
            }
            let mode = unsafe { &*snp.mode };
            let hw_addr_size = mode.hw_address_size as usize;
            if hw_addr_size < 6 { continue; }
            let mac: [u8; 6] = [
                mode.current_address.addr[0],
                mode.current_address.addr[1],
                mode.current_address.addr[2],
                mode.current_address.addr[3],
                mode.current_address.addr[4],
                mode.current_address.addr[5],
            ];
            w16(con_out, "Network adapter (SNP):\r\n");
            w16(con_out, "  MAC: ");
            print_mac(con_out, &mac);
            w16(con_out, "\r\n");
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
            snp_handled = true;
        }
    }

    if !snp_handled {
        w16(con_out, "SNP not available, trying direct e1000...\r\n");
        let _ = scan_e1000_devices(con_out, gbs, image_handle);
    }

    unsafe { free_pool(handle_buffer as *mut c_void); }
}

#[cfg(test)]
fn build_dhcp_response_frame(
    mac: &[u8; 6],
    xid: u32,
    msg_type: u8,
    yiaddr: [u8; 4],
    subnet: Option<[u8; 4]>,
    gateway: Option<[u8; 4]>,
) -> [u8; 1514] {
    let mut dhcp = make_dhcp_discover(xid, mac);
    dhcp[16..20].copy_from_slice(&yiaddr);
    dhcp[240] = 53; dhcp[241] = 1; dhcp[242] = msg_type;
    let mut off = 243;
    if let Some(s) = subnet {
        dhcp[off] = 1; dhcp[off + 1] = 4;
        dhcp[off + 2..off + 6].copy_from_slice(&s);
        off += 6;
    }
    if let Some(g) = gateway {
        dhcp[off] = 3; dhcp[off + 1] = 4;
        dhcp[off + 2..off + 6].copy_from_slice(&g);
        off += 6;
    }
    dhcp[off] = 255;
    let mut frame = [0u8; 1514];
    build_eth_ip_udp_dhcp(mac, &dhcp, 240, &mut frame);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ip_checksum() {
        let buf = [0x45, 0x00, 0x00, 0x2E, 0x00, 0x00, 0x00, 0x00, 0x40, 0x11, 0x00, 0x00, 0xC0, 0xA8, 0x01, 0x01, 0xC0, 0xA8, 0x01, 0x02];
        assert_eq!(ip_checksum(&buf), 0x8ED7);
    }

    #[test]
    fn test_ip_checksum_all_zeros() {
        assert_eq!(ip_checksum(&[0u8; 20]), 0xFFFF);
    }

    #[test]
    fn test_ip_checksum_all_ones() {
        let ones = [0xFFu8; 20];
        // (0xFFFF * 10) = 0x9FFF6, fold: 0xFFF6 + 9 = 0xFFFF, !0xFFFF = 0x0000
        assert_eq!(ip_checksum(&ones), 0x0000);
    }

    #[test]
    fn test_ip_checksum_odd_length() {
        // Odd number of bytes - last byte occupies high byte of word
        let buf = [0x12, 0x34, 0x56];
        // (0x1234 + 0x5600) = 0x6834, !0x6834 = 0x97CB
        assert_eq!(ip_checksum(&buf), 0x97CB);
    }

    #[test]
    fn test_ip_checksum_single_byte() {
        assert_eq!(ip_checksum(&[0xAB]), !(0xAB00u16));
    }

    #[test]
    fn test_make_dhcp_discover() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let pkt = make_dhcp_discover(0x12345678, &mac);
        assert_eq!(pkt[0], 1);
        assert_eq!(pkt[1], 1);
        assert_eq!(pkt[2], 6);
        assert_eq!(&pkt[4..8], &0x12345678u32.to_be_bytes());
        assert_eq!(&pkt[28..34], mac);
        assert_eq!(&pkt[236..240], &[0x63, 0x82, 0x53, 0x63]);
        assert_eq!(pkt[240], 53);
        assert_eq!(pkt[241], 1);
        assert_eq!(pkt[242], 1);
    }

    #[test]
    fn test_make_dhcp_discover_padding() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let pkt = make_dhcp_discover(0x12345678, &mac);
        // End marker at the option end
        assert_eq!(pkt[245], 255);
        // Padding after end marker should be zeros
        for i in 246..300 {
            assert_eq!(pkt[i], 0, "byte {} should be zero padding", i);
        }
    }

    #[test]
    fn test_build_eth_ip_udp_dhcp() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let mut dhcp_payload = [0u8; 300];
        dhcp_payload[0..240].fill(0xAA);
        let mut buf = [0u8; 1514];
        let len = build_eth_ip_udp_dhcp(&mac, &dhcp_payload, 240, &mut buf);
        assert_eq!(&buf[0..6], &[0xFFu8; 6]);
        assert_eq!(&buf[6..12], mac);
        assert_eq!(buf[12], 0x08);
        assert_eq!(buf[13], 0x00);
        assert_eq!(buf[14], 0x45);
        assert_eq!(len, 14 + 20 + 8 + 240);
        // UDP checksum should be 0 (not used)
        assert_eq!(buf[14 + 20 + 6], 0x00);
        assert_eq!(buf[14 + 20 + 7], 0x00);
    }

    // ── parse_dhcp_response success cases ──

    #[test]
    fn test_parse_dhcp_response_offer() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let frame = build_dhcp_response_frame(
            &mac, 0x12345678, 2, /* OFFER */
            [10, 0, 2, 15],
            Some([255, 255, 255, 0]),
            Some([10, 0, 2, 1]),
        );
        let cfg = parse_dhcp_response(&frame, frame.len(), 0x12345678, &mac).unwrap();
        assert_eq!(cfg.yiaddr, [10, 0, 2, 15]);
        assert_eq!(cfg.subnet, [255, 255, 255, 0]);
        assert_eq!(cfg.gateway, [10, 0, 2, 1]);
    }

    #[test]
    fn test_parse_dhcp_response_ack() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let frame = build_dhcp_response_frame(
            &mac, 0x12345678, 5, /* ACK */
            [192, 168, 1, 100],
            Some([255, 255, 255, 0]),
            Some([192, 168, 1, 1]),
        );
        let cfg = parse_dhcp_response(&frame, frame.len(), 0x12345678, &mac).unwrap();
        assert_eq!(cfg.yiaddr, [192, 168, 1, 100]);
    }

    #[test]
    fn test_parse_dhcp_response_no_gateway() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let frame = build_dhcp_response_frame(
            &mac, 0x12345678, 2,
            [10, 0, 2, 15],
            Some([255, 255, 255, 0]),
            None,
        );
        let cfg = parse_dhcp_response(&frame, frame.len(), 0x12345678, &mac).unwrap();
        assert_eq!(cfg.gateway, [0, 0, 0, 0]);
    }

    #[test]
    fn test_parse_dhcp_response_no_subnet() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let frame = build_dhcp_response_frame(
            &mac, 0x12345678, 2,
            [10, 0, 2, 15],
            None,
            None,
        );
        let cfg = parse_dhcp_response(&frame, frame.len(), 0x12345678, &mac).unwrap();
        assert_eq!(cfg.subnet, [255, 255, 255, 255]);
    }

    // ── parse_dhcp_response error cases ──

    #[test]
    fn test_parse_dhcp_response_too_short() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        assert!(parse_dhcp_response(&[0u8; 200], 200, 0x12345678, &mac).is_none());
    }

    #[test]
    fn test_parse_dhcp_response_wrong_ethertype() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let mut frame = build_dhcp_response_frame(
            &mac, 0x12345678, 2,
            [10, 0, 2, 15],
            None, None,
        );
        frame[12] = 0x08; frame[13] = 0x06; // ARP instead of IP
        assert!(parse_dhcp_response(&frame, frame.len(), 0x12345678, &mac).is_none());
    }

    #[test]
    fn test_parse_dhcp_response_not_udp() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let mut frame = build_dhcp_response_frame(
            &mac, 0x12345678, 2,
            [10, 0, 2, 15],
            None, None,
        );
        frame[14 + 9] = 6; // TCP instead of UDP
        assert!(parse_dhcp_response(&frame, frame.len(), 0x12345678, &mac).is_none());
    }

    #[test]
    fn test_parse_dhcp_response_bad_cookie() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let mut frame = build_dhcp_response_frame(
            &mac, 0x12345678, 2,
            [10, 0, 2, 15],
            None, None,
        );
        let ip_off = 14;
        let ip_hdr_len = (frame[ip_off] & 0x0F) as usize * 4;
        let dhcp_off = ip_off + ip_hdr_len + 8;
        frame[dhcp_off + 236] = 0x00; // corrupt magic cookie
        assert!(parse_dhcp_response(&frame, frame.len(), 0x12345678, &mac).is_none());
    }

    #[test]
    fn test_parse_dhcp_response_wrong_xid() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let frame = build_dhcp_response_frame(
            &mac, 0x12345678, 2,
            [10, 0, 2, 15],
            None, None,
        );
        assert!(parse_dhcp_response(&frame, frame.len(), 0xDEADBEEF, &mac).is_none());
    }

    #[test]
    fn test_parse_dhcp_response_wrong_mac() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        let other_mac = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let frame = build_dhcp_response_frame(
            &mac, 0x12345678, 2,
            [10, 0, 2, 15],
            None, None,
        );
        assert!(parse_dhcp_response(&frame, frame.len(), 0x12345678, &other_mac).is_none());
    }

    #[test]
    fn test_parse_dhcp_response_no_dhcp_msg_type() {
        let mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        // Frame with only end marker (255) at option start — no DHCP message type
        let mut dhcp = make_dhcp_discover(0x12345678, &mac);
        dhcp[240] = 255; // End marker immediately after cookie
        let mut frame = [0u8; 1514];
        build_eth_ip_udp_dhcp(&mac, &dhcp, 240, &mut frame);
        assert!(parse_dhcp_response(&frame, frame.len(), 0x12345678, &mac).is_none());
    }
}