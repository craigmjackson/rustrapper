#include "print.h"
#include <stdint.h>

#pragma GCC diagnostic ignored "-Warray-bounds"
#pragma GCC diagnostic ignored "-Wmaybe-uninitialized"
#pragma GCC diagnostic ignored "-Wunused-function"

// I/O port access (32-bit)
static inline uint32_t inl(uint16_t port)
{
    uint32_t val;
    __asm__ volatile("inl %1, %0" : "=a"(val) : "d"(port));
    return val;
}
static inline void outl(uint32_t val, uint16_t port)
{
    __asm__ volatile("outl %0, %1" : : "a"(val), "d"(port));
}

// Fixed buffer at 0x2000 from DS=0x0100 (physical 0x3000, above stage2 binary).
// Stage2 is loaded at physical 0x1000, max size 0x1B60 bytes → ends at 0x2B5F.
#define RECV_BUF ((uint8_t *)(unsigned long)0x2000)

static uint16_t htons(uint16_t v) { return (v << 8) | (v >> 8); }

static void print_mac(const uint8_t *mac)
{
    int i;
    for (i = 0; i < 6; i++) {
        if (i > 0) putc(':');
        uint8_t hi = mac[i] >> 4;
        uint8_t lo = mac[i] & 0x0F;
        putc(hi < 10 ? '0' + hi : 'A' + hi - 10);
        putc(lo < 10 ? '0' + lo : 'A' + lo - 10);
    }
}

static void print_ip(const uint8_t *ip)
{
    print_dec(ip[0]);
    putc('.');
    print_dec(ip[1]);
    putc('.');
    print_dec(ip[2]);
    putc('.');
    print_dec(ip[3]);
}

static int parse_dhcp_response(int len, uint32_t xid,
                               const uint8_t *mac,
                               uint8_t *yiaddr, uint8_t *subnet, uint8_t *gateway)
{
    int i;
    const uint8_t *buf = RECV_BUF;
    // UDP payload = raw DHCP message (no Ethernet/IP/UDP headers)
    if (len < 282) return -1;
    if (buf[236] != 0x63 || buf[237] != 0x82 ||
        buf[238] != 0x53 || buf[239] != 0x63)
        return -1;
    uint32_t pkt_xid = ((uint32_t)buf[4] << 24) |
                       ((uint32_t)buf[5] << 16) |
                       ((uint32_t)buf[6] << 8)  |
                        (uint32_t)buf[7];
    if (pkt_xid != xid) return -1;
    for (i = 0; i < 6; i++)
        if (buf[28 + i] != mac[i]) return -1;
    for (i = 0; i < 4; i++) yiaddr[i] = buf[16 + i];

    subnet[0] = 255; subnet[1] = 255; subnet[2] = 255; subnet[3] = 255;
    gateway[0] = 0; gateway[1] = 0; gateway[2] = 0; gateway[3] = 0;
    uint8_t dhcp_msg_type = 0;
    int off = 240;

    while (off + 1 < len) {
        uint8_t opt_type = buf[off];
        if (opt_type == 255) break;
        int opt_len = buf[off + 1];
        if (off + 2 + opt_len > len) break;
        if (opt_type == 53 && opt_len == 1)
            dhcp_msg_type = buf[off + 2];
        else if (opt_type == 1 && opt_len == 4)
            for (i = 0; i < 4; i++) subnet[i] = buf[off + 2 + i];
        else if (opt_type == 3 && opt_len >= 4)
            for (i = 0; i < 4; i++) gateway[i] = buf[off + 2 + i];
        off += 2 + opt_len;
    }
    return (dhcp_msg_type == 2 || dhcp_msg_type == 5) ? 0 : -1;
}

// ── PXE / UNDI fallback (for real hardware with proper PXE ROMs) ──

static uint16_t ds_seg(void)
{
    uint16_t seg;
    __asm__ ("mov %%ds, %0" : "=r"(seg));
    return seg;
}

// PXE parameter buffer must be in .data (--oformat=binary skips .bss)
static uint8_t pxe_buf[64] __attribute__((section(".data")));

static void pxe_clear(void)
{
    int i;
    for (i = 0; i < 64; i++) pxe_buf[i] = 0;
}

static uint16_t pxe_api(uint16_t function, uint16_t di, uint16_t es_seg)
{
    uint16_t ax;
    __asm__ volatile (
        "pushw %%ds\n"
        "pushw %%bx\n"
        "mov %2, %%es\n"
        "mov %3, %%bx\n"
        "xor %%ax, %%ax\n"
        "int $0x1A\n"
        "popw %%bx\n"
        "popw %%ds\n"
        : "=a"(ax)
        : "D"(di), "r"(es_seg), "m"(function)
        : "cx", "dx", "si", "cc", "memory"
    );
    return ax;
}

static int pxe_call(uint16_t func)
{
    uint16_t di = (uint16_t)(unsigned long)pxe_buf;
    return (int)(int16_t)pxe_api(func, di, ds_seg());
}

static int pxe_detect(void)
{
    uint16_t ax = 0x5650;
    uint8_t cf;
    __asm__ volatile (
        "pushw %%bx\npushw %%es\nint $0x1A\nsetc %1\npopw %%es\npopw %%bx\n"
        : "+a"(ax), "=qm"(cf)
        : : "cx", "dx", "si", "di", "cc", "memory"
    );
    return (cf || ax != 0x564E) ? -1 : 0;
}

// ── Direct e1000 driver via PCI I/O BAR ──
//
// NOTE: QEMU's e1000 I/O BAR is an empty stub (returns 0, ignores writes).
// On QEMU this path will fail with "Link timeout" and fall back to PXE/UNDI.
// On real hardware with a proper I/O BAR software access protocol it works.

#define E1000_VENDOR 0x8086
#define E1000_DEVICE 0x100E

// e1000 register offsets (accessible via I/O BAR software access)
#define E1000_CTRL     0x0000
#define E1000_STATUS   0x0008
#define E1000_EECD     0x0010
#define E1000_RA       0x5400
#define E1000_RDBAL    0x2800
#define E1000_RDBAH    0x2804
#define E1000_RDLEN    0x2808
#define E1000_RDH      0x2810
#define E1000_RDT      0x2818
#define E1000_RCTL     0x0100
#define E1000_TDBAL    0x3800
#define E1000_TDBAH    0x3804
#define E1000_TDLEN    0x3808
#define E1000_TDH      0x3810
#define E1000_TDT      0x3818
#define E1000_TCTL     0x0400
#define E1000_TIPG     0x0410
#define E1000_MTA      0x5200

// CTRL bits
#define E1000_CTRL_RST   (1 << 26)
#define E1000_CTRL_SLU   (1 << 6)
#define E1000_CTRL_FD    (1 << 0)

// STATUS bits
#define E1000_STATUS_LU  (1 << 1)

// RCTL bits
#define E1000_RCTL_EN    (1 << 1)
#define E1000_RCTL_SBP   (1 << 0)
#define E1000_RCTL_BAM   (1 << 15)
#define E1000_RCTL_BSIZE_2048 (0 << 16)

// TCTL bits
#define E1000_TCTL_EN    (1 << 1)
#define E1000_TCTL_PSP   (1 << 3)
#define E1000_TCTL_CT    (0x0F << 4)
#define E1000_TCTL_COLD  (0x40 << 12)

// e1000 descriptor (legacy, 16 bytes)
struct e1000_tx_desc {
    uint64_t addr;
    uint16_t length;
    uint8_t  cso;
    uint8_t  cmd;
    uint8_t  status;
    uint8_t  css;
    uint16_t special;
} __attribute__((packed));

struct e1000_rx_desc {
    uint64_t addr;
    uint16_t length;
    uint8_t  csum;
    uint8_t  status;
    uint8_t  errors;
    uint8_t  special;
    uint8_t  reserved[2];
} __attribute__((packed));

// TX descriptor command bits
#define E1000_TXD_CMD_EOP  (1 << 0)
#define E1000_TXD_CMD_IFCS (1 << 1)
#define E1000_TXD_CMD_RS   (1 << 3)

// TX descriptor status bits
#define E1000_TXD_STAT_DD  (1 << 0)

// RX descriptor status bits
#define E1000_RXD_STAT_DD  (1 << 0)
#define E1000_RXD_STAT_EOP (1 << 1)

// I/O BAR software access registers
// port_base+0: register address (write)
// port_base+4: register data (read/write)

static uint16_t g_io_base;

static void e1000_write(uint32_t reg, uint32_t val)
{
    outl(reg, g_io_base);
    outl(val, g_io_base + 4);
}

static uint32_t e1000_read(uint32_t reg)
{
    outl(reg, g_io_base);
    return inl(g_io_base + 4);
}

static int e1000_find(void)
{
    // Check PCI BIOS presence via INT 1A AH=B1h AL=01h
    uint32_t pci_sig;
    uint8_t cf;
    __asm__ volatile (
        "mov $0xB101, %%ax\n"
        "int $0x1A\n"
        "setc %0\n"
        : "=m"(cf), "=d"(pci_sig)
        :
        : "ax", "bx", "cx", "cc"
    );
    if (cf || pci_sig != 0x20494350) {
        puts("  PCI BIOS not found\r\n");
        return -1;
    }

    // Find first e1000 device: INT 1A AH=B1h AL=02h
    uint16_t bx;
    __asm__ volatile (
        "mov $0xB102, %%ax\n"
        "mov $0x100E, %%cx\n"        // E1000_DEVICE
        "mov $0x8086, %%dx\n"        // E1000_VENDOR
        "xor %%si, %%si\n"           // index = 0
        "int $0x1A\n"
        "setc %0\n"
        "mov %%bx, %1\n"
        : "=m"(cf), "=r"(bx)
        :
        : "ax", "cx", "dx", "si", "di", "cc"
    );
    if (cf) {
        puts("  e1000 not found on PCI\r\n");
        return -1;
    }

    // Read BAR1 (I/O ports) via PCI config read: INT 1A AH=B1h AL=09h
    uint32_t bar1;
    __asm__ volatile (
        "mov $0xB109, %%ax\n"
        "mov %2, %%bx\n"
        "mov $0x14, %%di\n"
        "int $0x1A\n"
        "setc %0\n"
        "mov %%ecx, %1\n"
        : "=m"(cf), "=r"(bar1)
        : "m"(bx)
        : "ax", "cx", "dx", "di", "cc"
    );
    if (cf || !(bar1 & 1)) {
        puts("  e1000 has no I/O BAR\r\n");
        return -1;
    }

    g_io_base = (uint16_t)(bar1 & ~3);
    putc(' ');
    print_hex(g_io_base, 4);
    puts("\r\n");
    return 0;
}

// Descriptor buffers: placed in .data for --oformat=binary compatibility
// TX/RX descriptors at 0x2600 (physical), past code+buffers
#define TX_DESC    ((struct e1000_tx_desc *)(unsigned long)0x2600)
#define RX_DESC    ((struct e1000_rx_desc *)(unsigned long)0x2610)

// TX/RX packet buffers at 0x2620 (physical), must not overlap other data
#define TX_BUF     ((uint8_t *)(unsigned long)0x2620)
#define RX_BUF     ((uint8_t *)(unsigned long)0x2720)

static int e1000_init(uint8_t *mac)
{
    int i;
    uint32_t tmp;

    // Reset NIC
    e1000_write(E1000_CTRL, E1000_CTRL_RST);
    for (i = 0; i < 10000; i++) {
        tmp = e1000_read(E1000_CTRL);
        if (!(tmp & E1000_CTRL_RST)) break;
    }
    if (tmp & E1000_CTRL_RST) {
        puts("  e1000 reset timeout\r\n");
        return -1;
    }

    // Set link up, full duplex
    tmp = e1000_read(E1000_CTRL);
    tmp |= E1000_CTRL_SLU | E1000_CTRL_FD;
    e1000_write(E1000_CTRL, tmp);

    // Wait for link
    {
        int link_up = 0;
        for (i = 0; i < 50000; i++) {
            if (e1000_read(E1000_STATUS) & E1000_STATUS_LU) {
                link_up = 1;
                break;
            }
        }
    if (!link_up) {
        puts("  Link timeout (QEMU I/O BAR stub - expected, trying PXE...)\r\n");
            return -1;
        }
    }

    // Read MAC from RA register (0x5400)
    uint32_t ra_low = e1000_read(E1000_RA);
    uint32_t ra_high = e1000_read(E1000_RA + 4);
    mac[0] = (uint8_t)(ra_low);
    mac[1] = (uint8_t)(ra_low >> 8);
    mac[2] = (uint8_t)(ra_low >> 16);
    mac[3] = (uint8_t)(ra_low >> 24);
    mac[4] = (uint8_t)(ra_high);
    mac[5] = (uint8_t)(ra_high >> 8);

    // Set up RX descriptor
    __builtin_memset(RX_DESC, 0, sizeof(*RX_DESC));
    RX_DESC->addr = (uint64_t)(unsigned long)RX_BUF;
    e1000_write(E1000_RDBAL, (uint32_t)(unsigned long)RX_DESC);
    e1000_write(E1000_RDBAH, 0);
    e1000_write(E1000_RDLEN, sizeof(struct e1000_rx_desc));
    e1000_write(E1000_RDH, 0);
    e1000_write(E1000_RDT, 0);

    // Set up TX descriptor
    __builtin_memset(TX_DESC, 0, sizeof(*TX_DESC));
    TX_DESC->addr = (uint64_t)(unsigned long)TX_BUF;
    e1000_write(E1000_TDBAL, (uint32_t)(unsigned long)TX_DESC);
    e1000_write(E1000_TDBAH, 0);
    e1000_write(E1000_TDLEN, sizeof(struct e1000_tx_desc));
    e1000_write(E1000_TDH, 0);
    e1000_write(E1000_TDT, 0);

    // Enable receiver
    e1000_write(E1000_RCTL, E1000_RCTL_EN | E1000_RCTL_SBP | E1000_RCTL_BAM);

    // Enable transmitter
    e1000_write(E1000_TCTL, E1000_TCTL_EN | E1000_TCTL_PSP |
                E1000_TCTL_CT | E1000_TCTL_COLD);

    // Inter-packet gap
    e1000_write(E1000_TIPG, 0x0060200A);

    // Clear multicast table
    for (i = 0; i < 128; i++)
        e1000_write(E1000_MTA + i * 4, 0);

    puts("  Link up");
    return 0;
}

static int e1000_tx(int len, const uint8_t *data)
{
    int i;
    // Copy packet data to TX buffer
    for (i = 0; i < len; i++)
        TX_BUF[i] = data[i];
    for (; i < 60; i++)  // pad to min 60 bytes
        TX_BUF[i] = 0;

    // Fill TX descriptor
    TX_DESC->addr = (uint64_t)(unsigned long)TX_BUF;
    TX_DESC->length = (len < 60) ? 60 : len;
    TX_DESC->cmd = E1000_TXD_CMD_EOP | E1000_TXD_CMD_IFCS | E1000_TXD_CMD_RS;
    TX_DESC->status = 0;
    TX_DESC->css = 0;
    TX_DESC->cso = 0;

    // Ring doorbell
    e1000_write(E1000_TDT, 1);

    // Wait for transmission
    for (i = 0; i < 50000; i++) {
        if (TX_DESC->status & E1000_TXD_STAT_DD) break;
    }
    return (i < 50000) ? 0 : -1;
}

static int e1000_rx(void)
{
    if (RX_DESC->status & E1000_RXD_STAT_DD) {
        int len = (int)RX_DESC->length;
        __builtin_memset(RECV_BUF, 0, 1514);
        for (int i = 0; i < len && i < 1514; i++)
            RECV_BUF[i] = RX_BUF[i];
        // Re-arm descriptor
        RX_DESC->status = 0;
        e1000_write(E1000_RDT, 1);
        return len;
    }
    return 0;
}

// Build a full Ethernet/IP/UDP/DHCPDISCOVER frame
static int build_dhcp_frame(uint8_t *buf, uint32_t xid, const uint8_t *mac)
{
    int i;

    // Zero buffer
    for (i = 0; i < 300; i++) buf[i] = 0;

    // ── Ethernet header (14 bytes) ──
    for (i = 0; i < 6; i++) buf[i] = 0xFF;          // dst = broadcast
    for (i = 0; i < 6; i++) buf[6 + i] = mac[i];    // src = our MAC
    buf[12] = 0x08; buf[13] = 0x00;                  // EtherType = IPv4

    // ── IP header (20 bytes, at offset 14) ──
    int ip_off = 14;
    buf[ip_off + 0] = 0x45;                           // Version=4, IHL=5
    buf[ip_off + 8] = 0x80;                           // TTL
    buf[ip_off + 9] = 0x11;                           // Protocol = UDP
    for (i = 0; i < 4; i++) buf[ip_off + 16 + i] = 0xFF;  // dst IP = 255.255.255.255
    // src IP (ip_off+12..15) stays 0.0.0.0

    // ── UDP header (8 bytes, at offset 34) ──
    int udp_off = ip_off + 20;
    buf[udp_off + 0] = 0x00; buf[udp_off + 1] = 0x44;  // sport = 68
    buf[udp_off + 2] = 0x00; buf[udp_off + 3] = 0x43;  // dport = 67

    // ── DHCP payload (at offset 42) ──
    int dhcp_off = udp_off + 8;
    buf[dhcp_off + 0] = 1;                             // op = BOOTREQUEST
    buf[dhcp_off + 1] = 1;                             // htype = Ethernet
    buf[dhcp_off + 2] = 6;                             // hlen = 6
    buf[dhcp_off + 3] = 0;                             // hops = 0
    buf[dhcp_off + 4] = (uint8_t)(xid >> 24);
    buf[dhcp_off + 5] = (uint8_t)(xid >> 16);
    buf[dhcp_off + 6] = (uint8_t)(xid >> 8);
    buf[dhcp_off + 7] = (uint8_t)(xid);
    buf[dhcp_off + 10] = 0x80;                         // flags = broadcast
    for (i = 0; i < 6; i++)
        buf[dhcp_off + 28 + i] = mac[i];               // chaddr = MAC

    // DHCP magic cookie
    buf[dhcp_off + 236] = 0x63;
    buf[dhcp_off + 237] = 0x82;
    buf[dhcp_off + 238] = 0x53;
    buf[dhcp_off + 239] = 0x63;

    // DHCP options
    buf[dhcp_off + 240] = 53;  buf[dhcp_off + 241] = 1;  buf[dhcp_off + 242] = 1;  // DHCPDISCOVER
    buf[dhcp_off + 243] = 55;  buf[dhcp_off + 244] = 3;  buf[dhcp_off + 245] = 1;
    buf[dhcp_off + 246] = 3;   buf[dhcp_off + 247] = 6;
    buf[dhcp_off + 248] = 255;  // end

    int dhcp_len = 249;
    int udp_len = 8 + dhcp_len;
    int total_len = 20 + udp_len;

    // Fill total length in IP header (buf[16-17])
    buf[ip_off + 2] = (uint8_t)(total_len >> 8);
    buf[ip_off + 3] = (uint8_t)(total_len);

    // Fill UDP length (buf[38-39])
    buf[udp_off + 4] = (uint8_t)(udp_len >> 8);
    buf[udp_off + 5] = (uint8_t)(udp_len);

    // IP header checksum
    uint32_t sum = 0;
    for (i = 0; i < 20; i += 2) {
        if (i == 10) continue;  // skip checksum field at offset 10-11
        sum += (buf[ip_off + i] << 8) | buf[ip_off + i + 1];
    }
    while (sum >> 16) sum = (sum & 0xFFFF) + (sum >> 16);
    uint16_t cksum = (uint16_t)(~sum & 0xFFFF);
    buf[ip_off + 10] = (uint8_t)(cksum >> 8);
    buf[ip_off + 11] = (uint8_t)(cksum);

    return 42 + dhcp_len;
}

static int e1000_scan(void)
{
    uint8_t mac[6];
    uint8_t yiaddr[4];
    uint8_t subnet[4];
    uint8_t gateway[4];
    int found = 0;
    int i;

    puts("Scanning PCI for e1000...\r\n");

    if (e1000_find() != 0) {
        puts("  No e1000 found.\r\n");
        return -1;
    }

    if (e1000_init(mac) != 0) {
        puts("  e1000 init failed (QEMU I/O stub - trying PXE/UNDI...)\r\n");
        return -1;
    }

    puts("  MAC: ");
    print_mac(mac);
    puts("\r\n");

    uint32_t xid = 0x12345678;
    int frame_len = build_dhcp_frame(TX_BUF, xid, mac);

    puts("  DHCP: Sending DISCOVER...");
    if (e1000_tx(frame_len, TX_BUF) != 0) {
        puts("send failed\r\n");
        return -1;
    }
    puts("sent, waiting for OFFER...\r\n");

    for (i = 0; i < 100000; i++) {
        int len = e1000_rx();
        if (len <= 0) {
            int j;
            for (j = 0; j < 200; j++) __asm__ ("pause");
            continue;
        }

        if (parse_dhcp_response(len, xid, mac, yiaddr, subnet, gateway) == 0) {
            found = 1;
            break;
        }
    }

    if (!found) {
        puts("  DHCP: timeout\r\n");
    } else {
        puts("  IP: ");
        print_ip(yiaddr);
        puts("\r\n  Subnet: ");
        print_ip(subnet);
        puts("\r\n  Gateway: ");
        if (gateway[0] == 0 && gateway[1] == 0 &&
            gateway[2] == 0 && gateway[3] == 0)
            puts("(none)");
        else
            print_ip(gateway);
        puts("\r\n");
    }

    return found ? 0 : -1;
}

// Entry point: try direct e1000 first, fall back to PXE/UNDI
int pxe_scan(void)
{
    puts("Scanning for network adapters...\r\n");
    if (e1000_scan() == 0) return 0;

    // Fallback: PXE/UNDI (works with real hardware PXE ROMs)
    puts("  Trying PXE/UNDI...\r\n");
    if (pxe_detect() != 0) {
        puts("  No PXE stack found.\r\n");
        return -1;
    }

    uint8_t mac[6];
    uint8_t yiaddr[4];
    uint8_t subnet[4];
    uint8_t gateway[4];
    pxe_clear();
    if (pxe_call(0x0001) != 0) { puts("  UNDI_STARTUP failed\r\n"); return -1; }

    pxe_clear();
    if (pxe_call(0x0003) != 0) { /* non-fatal */ }

    // Read MAC (6 bytes) + IP (4 bytes) from 0x500 and 0x506
    {
        uint16_t raw[5];
        int i;
        __asm__ (
            "pushw %%es\n"
            "xor %%ax, %%ax\n"
            "mov %%ax, %%es\n"
            "mov %%es:(0x500), %0\n"
            "mov %%es:(0x502), %1\n"
            "mov %%es:(0x504), %2\n"
            "mov %%es:(0x506), %3\n"
            "mov %%es:(0x508), %4\n"
            "popw %%es\n"
            : "=r"(raw[0]), "=r"(raw[1]), "=r"(raw[2]),
              "=r"(raw[3]), "=r"(raw[4])
            : : "ax", "cc"
        );
        for (i = 0; i < 6; i++) mac[i] = ((uint8_t*)raw)[i];
        for (i = 0; i < 4; i++) yiaddr[i] = ((uint8_t*)raw)[6 + i];
    }
    // Read subnet (4 bytes) + gateway (4 bytes) from 0x50A and 0x50E
    {
        uint16_t raw[4];
        int i;
        __asm__ (
            "pushw %%es\n"
            "xor %%ax, %%ax\n"
            "mov %%ax, %%es\n"
            "mov %%es:(0x50A), %0\n"
            "mov %%es:(0x50C), %1\n"
            "mov %%es:(0x50E), %2\n"
            "mov %%es:(0x510), %3\n"
            "popw %%es\n"
            : "=r"(raw[0]), "=r"(raw[1]), "=r"(raw[2]), "=r"(raw[3])
            : : "ax", "cc"
        );
        for (i = 0; i < 4; i++) subnet[i] = ((uint8_t*)raw)[i];
        for (i = 0; i < 4; i++) gateway[i] = ((uint8_t*)raw)[4 + i];
    }

    puts("  MAC: ");
    print_mac(mac);
    puts("\r\n");

    // Check if IP is non-zero (DHCP succeeded during iPXE boot)
    {
        int i, has_ip = 0;
        for (i = 0; i < 4; i++) if (yiaddr[i] != 0) has_ip = 1;
        if (!has_ip) {
            puts("  No IP from iPXE DHCP\r\n");
            return -1;
        }
    }

    puts("  IP: ");
    print_ip(yiaddr);
    puts("\r\n  Subnet: ");
    print_ip(subnet);
    puts("\r\n  Gateway: ");
    if (gateway[0] == 0 && gateway[1] == 0 &&
        gateway[2] == 0 && gateway[3] == 0)
        puts("(none)");
    else
        print_ip(gateway);
    puts("\r\n");

    return 0;
}
