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
    puts("sent\n");

    let mut buf = [0u8; 1514];
    let copy_len = e1000_common::try_receive(base, &mut buf, 100_000_000)?;
    dhcp::parse_response(&buf, copy_len, xid, mac)
}
