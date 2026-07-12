use core::ptr::{read_volatile, write_volatile};

use common::print::{print_dec, print_hex, putc, puts};

use crate::pci;

// e1000 registers (offsets from BAR0)
const REG_CTRL: u32 = 0x0000;
const REG_STATUS: u32 = 0x0008;
const REG_RCTL: u32 = 0x0100;
const REG_TCTL: u32 = 0x0400;
const REG_RDBAL: u32 = 0x2800;
const REG_RDBAH: u32 = 0x2804;
const REG_RDLEN: u32 = 0x2808;
const REG_RDH: u32 = 0x2810;
const REG_RDT: u32 = 0x2818;
const REG_TDBAL: u32 = 0x3800;
const REG_TDBAH: u32 = 0x3804;
const REG_TDLEN: u32 = 0x3808;
const REG_TDH: u32 = 0x3810;
const REG_TDT: u32 = 0x3818;
const REG_RA: u32 = 0x5400;
const REG_MTA: u32 = 0x5200;

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

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[derive(Clone, Copy)]
#[repr(C, packed)]
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

static mut RX_DESCS: RxDescs = RxDescs([RxDesc {
    addr: 0,
    length: 0,
    checksum: 0,
    status: 0,
    errors: 0,
    special: 0,
}; NUM_RX_DESC]);
static mut TX_DESCS: TxDescs = TxDescs([TxDesc {
    addr: 0,
    length: 0,
    cso: 0,
    cmd: 0,
    status: 0,
    css: 0,
    special: 0,
}; NUM_TX_DESC]);

static mut RX_BUF: [u8; RX_BUFFER_SIZE] = [0u8; RX_BUFFER_SIZE];
static mut TX_BUF: [u8; 2048] = [0u8; 2048];

fn reg_read32(base: u32, reg: u32) -> u32 {
    unsafe { read_volatile((base + reg) as *const u32) }
}

fn reg_write32(base: u32, reg: u32, val: u32) {
    unsafe { write_volatile((base + reg) as *mut u32, val) }
}

fn e1000_read_mac(base: u32) -> [u8; 6] {
    let low = reg_read32(base, REG_RA);
    let high = reg_read32(base, REG_RA + 4);
    [
        low as u8,
        (low >> 8) as u8,
        (low >> 16) as u8,
        (low >> 24) as u8,
        high as u8,
        (high >> 8) as u8,
    ]
}

fn e1000_set_mac(base: u32, mac: &[u8; 6]) {
    let low = mac[0] as u32
        | (mac[1] as u32) << 8
        | (mac[2] as u32) << 16
        | (mac[3] as u32) << 24;
    let high = mac[4] as u32 | (mac[5] as u32) << 8;
    reg_write32(base, REG_RA, low);
    reg_write32(base, REG_RA + 4, high | 0x8000_0001);
}

fn e1000_clear_multicast(base: u32) {
    for i in 0..128 {
        reg_write32(base, REG_MTA + i * 4, 0);
    }
}

fn e1000_init(base: u32) -> bool {
    reg_write32(base, REG_CTRL, reg_read32(base, REG_CTRL) | CTRL_RST);
    for _ in 0..100000 {
        if reg_read32(base, REG_CTRL) & CTRL_RST == 0 {
            break;
        }
        core::hint::spin_loop();
    }

    for _ in 0..1000000 {
        if reg_read32(base, REG_STATUS) & STATUS_LU != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    let mac = e1000_read_mac(base);
    if mac == [0u8; 6] || mac == [0xFFu8; 6] {
        return false;
    }

    e1000_set_mac(base, &mac);
    e1000_clear_multicast(base);

    unsafe {
        let rx_buf_addr = core::ptr::addr_of!(RX_BUF) as *const u8 as u32;
        for i in 0..NUM_RX_DESC {
            let desc = &raw mut RX_DESCS.0[i];
            core::ptr::write_volatile(core::ptr::addr_of_mut!((*desc).addr), rx_buf_addr as u64);
        }
        let tx_desc = &raw mut TX_DESCS.0[0];
        core::ptr::write_volatile(
            core::ptr::addr_of_mut!((*tx_desc).addr),
            core::ptr::addr_of!(TX_BUF) as *const u8 as u64,
        );
    }

    let rdbal = core::ptr::addr_of!(RX_DESCS) as u32;
    reg_write32(base, REG_RDBAL, rdbal);
    reg_write32(base, REG_RDBAH, 0);
    reg_write32(base, REG_RDLEN, (NUM_RX_DESC * 16) as u32);
    reg_write32(base, REG_RDH, 0);
    reg_write32(base, REG_RDT, (NUM_RX_DESC - 1) as u32);

    let tdbal = core::ptr::addr_of!(TX_DESCS) as u32;
    reg_write32(base, REG_TDBAL, tdbal);
    reg_write32(base, REG_TDBAH, 0);
    reg_write32(base, REG_TDLEN, (NUM_TX_DESC * 16) as u32);
    reg_write32(base, REG_TDH, 0);
    reg_write32(base, REG_TDT, 0);

    let rctl = RCTL_EN | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_SECRC | (0 << RCTL_BSIZE_SHIFT);
    reg_write32(base, REG_RCTL, rctl);

    let tctl = TCTL_EN | TCTL_PSP | (0x0F << TCTL_CT_SHIFT) | (0x3F << TCTL_COLD_SHIFT);
    reg_write32(base, REG_TCTL, tctl);

    reg_write32(base, REG_CTRL, reg_read32(base, REG_CTRL) | CTRL_SLU | CTRL_FD);

    for _ in 0..1000000 {
        if reg_read32(base, REG_STATUS) & STATUS_LU != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    puts("      RDBAL=0x");
    print_hex(reg_read32(base, REG_RDBAL) as u64, 8);
    puts(" RDLEN=0x");
    print_hex(reg_read32(base, REG_RDLEN) as u64, 8);
    puts(" RDH=0x");
    print_hex(reg_read32(base, REG_RDH) as u64, 4);
    puts(" RDT=0x");
    print_hex(reg_read32(base, REG_RDT) as u64, 4);
    putc(b'\n');
    puts("      TDBAL=0x");
    print_hex(reg_read32(base, REG_TDBAL) as u64, 8);
    puts(" TDLEN=0x");
    print_hex(reg_read32(base, REG_TDLEN) as u64, 8);
    puts(" TDH=0x");
    print_hex(reg_read32(base, REG_TDH) as u64, 4);
    puts(" TDT=0x");
    print_hex(reg_read32(base, REG_TDT) as u64, 4);
    putc(b'\n');

    true
}

fn e1000_send(base: u32, data: &[u8]) -> bool {
    if data.len() > 2048 {
        return false;
    }

    unsafe {
        let buf = core::ptr::addr_of_mut!(TX_BUF) as *mut u8;
        for i in 0..data.len() {
            *buf.add(i) = data[i];
        }
        let desc = &raw mut TX_DESCS.0[0];
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*desc).length), data.len() as u16);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*desc).cmd), CMD_EOP | CMD_IFCS | CMD_RS);
        core::ptr::write_volatile(core::ptr::addr_of_mut!((*desc).status), 0u8);
    }

    let old_tdt = reg_read32(base, REG_TDT);
    reg_write32(base, REG_TDT, old_tdt.wrapping_add(1));

    for _ in 0..2000000 {
        let status = unsafe {
            let desc = &raw const TX_DESCS.0[0];
            core::ptr::read_volatile(core::ptr::addr_of!((*desc).status))
        };
        if status & TX_STATUS_DD != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    let ok = unsafe {
        let desc = &raw const TX_DESCS.0[0];
        core::ptr::read_volatile(core::ptr::addr_of!((*desc).status)) & TX_STATUS_DD != 0
    };

    ok
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

fn make_dhcp_discover(xid: u32, mac: &[u8; 6]) -> [u8; 300] {
    let mut pkt = [0u8; 300];
    pkt[0] = 1;
    pkt[1] = 1;
    pkt[2] = 6;
    pkt[4..8].copy_from_slice(&xid.to_be_bytes());
    pkt[10] = 0x80;
    pkt[28..34].copy_from_slice(mac);
    pkt[236..240].copy_from_slice(&[0x63, 0x82, 0x53, 0x63]);

    let mut off = 240;
    pkt[off] = 53;
    pkt[off + 1] = 1;
    pkt[off + 2] = 1;
    off += 3;
    pkt[off] = 55;
    pkt[off + 1] = 3;
    pkt[off + 2] = 1;
    pkt[off + 3] = 3;
    pkt[off + 4] = 6;
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
    buf[12] = 0x08;
    buf[13] = 0x00;

    let ip_off = 14usize;
    let ip_total_len = 20 + 8 + dhcp_len;

    buf[ip_off] = 0x45;
    buf[ip_off + 1] = 0x00;
    buf[ip_off + 2..ip_off + 4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    buf[ip_off + 8] = 64;
    buf[ip_off + 9] = 17;
    buf[ip_off + 12..ip_off + 16].fill(0x00);
    buf[ip_off + 16..ip_off + 20].fill(0xFF);

    let cksum = ip_checksum(&buf[ip_off..ip_off + 20]);
    buf[ip_off + 10..ip_off + 12].copy_from_slice(&cksum.to_be_bytes());

    let udp_off = ip_off + 20;
    let udp_len = 8 + dhcp_len;
    buf[udp_off..udp_off + 2].copy_from_slice(&[0x00, 0x44]);
    buf[udp_off + 2..udp_off + 4].copy_from_slice(&[0x00, 0x43]);
    buf[udp_off + 4..udp_off + 6].copy_from_slice(&(udp_len as u16).to_be_bytes());

    let dhcp_off = udp_off + 8;
    buf[dhcp_off..dhcp_off + dhcp_len].copy_from_slice(&dhcp_payload[..dhcp_len]);
    dhcp_off + dhcp_len
}

struct DhcpConfig {
    yiaddr: [u8; 4],
    subnet: [u8; 4],
    gateway: [u8; 4],
}

fn parse_dhcp_response(buf: &[u8], len: usize, xid: u32, mac: &[u8; 6]) -> Option<DhcpConfig> {
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
    }

    let udp_off = ip_off + ip_hdr_len;
    let dhcp_off = udp_off + 8;
    let dhcp_len = len - dhcp_off;
    if dhcp_len < 240 {
        return None;
    }
    if dhcp_off + 4 > len {
        return None;
    }
    if buf[dhcp_off + 236] != 0x63
        || buf[dhcp_off + 237] != 0x82
        || buf[dhcp_off + 238] != 0x53
        || buf[dhcp_off + 239] != 0x63
    {
        return None;
    }

    let pkt_xid = u32::from_be_bytes([
        buf[dhcp_off + 4],
        buf[dhcp_off + 5],
        buf[dhcp_off + 6],
        buf[dhcp_off + 7],
    ]);
    if pkt_xid != xid {
        return None;
    }

    for i in 0..6 {
        if buf[dhcp_off + 28 + i] != mac[i] {
            return None;
        }
    }

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

    if dhcp_msg_type != 2 && dhcp_msg_type != 5 {
        return None;
    }

    Some(DhcpConfig {
        yiaddr,
        subnet,
        gateway,
    })
}

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

fn print_ip(ip: &[u8; 4]) {
    print_dec(ip[0] as u64);
    putc(b'.');
    print_dec(ip[1] as u64);
    putc(b'.');
    print_dec(ip[2] as u64);
    putc(b'.');
    print_dec(ip[3] as u64);
}

fn dhcp_recv(base: u32, mac: &[u8; 6], xid: u32) -> Option<DhcpConfig> {
    let mut buf = [0u8; 1514];
    for _ in 0..100_000_000 {
        for idx in 0..NUM_RX_DESC {
            unsafe {
                let desc = &raw const RX_DESCS.0[idx];
                let status = core::ptr::read_volatile(core::ptr::addr_of!((*desc).status));
                if status & RX_STATUS_DD != 0 {
                    let len = core::ptr::read_volatile(core::ptr::addr_of!((*desc).length)) as usize;
                    let copy_len = if len > 1514 { 1514 } else { len };
                    core::ptr::copy_nonoverlapping(
                        core::ptr::addr_of!(RX_BUF) as *const u8,
                        buf.as_mut_ptr(),
                        copy_len,
                    );
                    let desc = &raw mut RX_DESCS.0[idx];
                    core::ptr::write_volatile(core::ptr::addr_of_mut!((*desc).status), 0u8);
                    reg_write32(base, REG_RDT, idx as u32);
                    if let Some(cfg) = parse_dhcp_response(&buf, copy_len, xid, mac) {
                        return Some(cfg);
                    }
                    break;
                }
            }
        }
        core::hint::spin_loop();
    }
    None
}

fn dhcp_run(base: u32, mac: &[u8; 6]) -> Option<DhcpConfig> {
    let xid: u32 = 0x12345678;

    puts("DHCP...");
    let discover = make_dhcp_discover(xid, mac);
    let mut frame = [0u8; 1514];
    let frame_len = build_eth_ip_udp_dhcp(mac, &discover, 300, &mut frame);
    if !e1000_send(base, &frame[..frame_len]) {
        puts("send failed\n");
        return None;
    }
    puts("sent\n");

    dhcp_recv(base, mac, xid)
}

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

        puts("    MMIO BAR0: 0x");
        print_hex(bar0 as u64, 8);
        putc(b'\n');

        if !e1000_init(bar0) {
            puts("    e1000 init failed\n");
            continue;
        }

        let mac = e1000_read_mac(bar0);
        puts("    MAC: ");
        print_mac(&mac);
        putc(b'\n');

        puts("    DHCP: ");
        match dhcp_run(bar0, &mac) {
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
            }
            None => {
                puts("FAILED\n");
            }
        }

        return;
    }

    puts("  No network adapters found.\n");
}
