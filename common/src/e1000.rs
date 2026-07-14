use core::ptr::{read_volatile, write_volatile};

// e1000 register offsets (from BAR0, u64 for cross-platform use)
pub const REG_CTRL: u64 = 0x0000;
pub const REG_STATUS: u64 = 0x0008;
pub const REG_RCTL: u64 = 0x0100;
pub const REG_TCTL: u64 = 0x0400;
pub const REG_RDBAL: u64 = 0x2800;
pub const REG_RDBAH: u64 = 0x2804;
pub const REG_RDLEN: u64 = 0x2808;
pub const REG_RDH: u64 = 0x2810;
pub const REG_RDT: u64 = 0x2818;
pub const REG_TDBAL: u64 = 0x3800;
pub const REG_TDBAH: u64 = 0x3804;
pub const REG_TDLEN: u64 = 0x3808;
pub const REG_TDH: u64 = 0x3810;
pub const REG_TDT: u64 = 0x3818;
pub const REG_RA: u64 = 0x5400;
pub const REG_MTA: u64 = 0x5200;

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

pub const NUM_RX_DESC: usize = 8;
pub const NUM_TX_DESC: usize = 8;

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

#[cfg(not(test))]
static mut RX_DESCS: RxDescs = RxDescs([RxDesc {
    addr: 0,
    length: 0,
    checksum: 0,
    status: 0,
    errors: 0,
    special: 0,
}; NUM_RX_DESC]);
#[cfg(not(test))]
static mut TX_DESCS: TxDescs = TxDescs([TxDesc {
    addr: 0,
    length: 0,
    cso: 0,
    cmd: 0,
    status: 0,
    css: 0,
    special: 0,
}; NUM_TX_DESC]);

#[cfg(not(test))]
static mut RX_BUF: [u8; RX_BUFFER_SIZE] = [0u8; RX_BUFFER_SIZE];
#[cfg(not(test))]
static mut TX_BUF: [u8; 2048] = [0u8; 2048];

/// MMIO register access. `base` is the BAR0 address cast to u64.
pub fn reg_read32(base: u64, reg: u64) -> u32 {
    unsafe { read_volatile((base + reg) as *const u32) }
}

/// MMIO register write. `base` is the BAR0 address cast to u64.
pub fn reg_write32(base: u64, reg: u64, val: u32) {
    unsafe { write_volatile((base + reg) as *mut u32, val) }
}

/// Read the MAC address from the NIC's Receive Address registers.
pub fn read_mac(base: u64) -> [u8; 6] {
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

fn set_mac(base: u64, mac: &[u8; 6]) {
    let low = mac[0] as u32
        | (mac[1] as u32) << 8
        | (mac[2] as u32) << 16
        | (mac[3] as u32) << 24;
    let high = mac[4] as u32 | (mac[5] as u32) << 8;
    reg_write32(base, REG_RA, low);
    reg_write32(base, REG_RA + 4, high | 0x8000_0001);
}

fn clear_multicast(base: u64) {
    for i in 0..128 {
        reg_write32(base, REG_MTA + i as u64 * 4, 0);
    }
}

/// Initialize the e1000 NIC at `base`. Returns the MAC address on success, `None` on failure.
/// All targets' `e1000_init` reduce to this single implementation.
pub fn init(base: u64) -> Option<[u8; 6]> {
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

    let mac = read_mac(base);
    if mac == [0u8; 6] || mac == [0xFFu8; 6] {
        return None;
    }

    set_mac(base, &mac);
    clear_multicast(base);

    #[cfg(not(test))]
    unsafe {
        let rx_buf_addr = core::ptr::addr_of!(RX_BUF) as *const u8 as u64;
        for i in 0..NUM_RX_DESC {
            let desc = &raw mut RX_DESCS.0[i];
            write_volatile(core::ptr::addr_of_mut!((*desc).addr), rx_buf_addr);
        }
        let tx_buf_addr = core::ptr::addr_of!(TX_BUF) as *const u8 as u64;
        for i in 0..NUM_TX_DESC {
            let desc = &raw mut TX_DESCS.0[i];
            write_volatile(core::ptr::addr_of_mut!((*desc).addr), tx_buf_addr);
        }
    }

    #[cfg(not(test))]
    let rdbal: u32 = core::ptr::addr_of!(RX_DESCS) as u64 as u32;
    #[cfg(test)]
    let rdbal: u32 = 0;
    reg_write32(base, REG_RDBAL, rdbal);
    reg_write32(base, REG_RDBAH, 0);
    reg_write32(base, REG_RDLEN, (NUM_RX_DESC * 16) as u32);
    reg_write32(base, REG_RDH, 0);
    reg_write32(base, REG_RDT, (NUM_RX_DESC - 1) as u32);

    #[cfg(not(test))]
    let tdbal: u32 = core::ptr::addr_of!(TX_DESCS) as u64 as u32;
    #[cfg(test)]
    let tdbal: u32 = 0;
    reg_write32(base, REG_TDBAL, tdbal);
    reg_write32(base, REG_TDBAH, 0);
    reg_write32(base, REG_TDLEN, (NUM_TX_DESC * 16) as u32);
    reg_write32(base, REG_TDH, 0);
    reg_write32(base, REG_TDT, 0);

    let rctl = RCTL_EN | RCTL_UPE | RCTL_MPE | RCTL_BAM | RCTL_SECRC
        | (0 << RCTL_BSIZE_SHIFT);
    reg_write32(base, REG_RCTL, rctl);

    let tctl = TCTL_EN | TCTL_PSP
        | (0x0F << TCTL_CT_SHIFT)
        | (0x3F << TCTL_COLD_SHIFT);
    reg_write32(base, REG_TCTL, tctl);

    reg_write32(base, REG_CTRL, reg_read32(base, REG_CTRL) | CTRL_SLU | CTRL_FD);

    for _ in 0..1000000 {
        if reg_read32(base, REG_STATUS) & STATUS_LU != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    Some(mac)
}

/// Send a frame via the e1000. Returns true on successful transmit.
/// Uses the TX descriptor at the current TDT index so multiple sequential
/// sends work correctly (each send advances TDT by 1).
pub fn send(base: u64, data: &[u8]) -> bool {
    if data.len() > 2048 {
        return false;
    }

    let tdt = (reg_read32(base, REG_TDT) as usize) % NUM_TX_DESC;

    #[cfg(not(test))]
    unsafe {
        let buf = core::ptr::addr_of_mut!(TX_BUF) as *mut u8;
        for i in 0..data.len() {
            *buf.add(i) = data[i];
        }
        let desc = &raw mut TX_DESCS.0[tdt];
        write_volatile(core::ptr::addr_of_mut!((*desc).length), data.len() as u16);
        write_volatile(core::ptr::addr_of_mut!((*desc).cmd), CMD_EOP | CMD_IFCS | CMD_RS);
        write_volatile(core::ptr::addr_of_mut!((*desc).status), 0u8);
    }

    reg_write32(base, REG_TDT, ((tdt + 1) % NUM_TX_DESC) as u32);

    for _ in 0..2000000 {
        #[cfg(not(test))]
        let status = unsafe {
            let desc = &raw const TX_DESCS.0[tdt];
            read_volatile(core::ptr::addr_of!((*desc).status))
        };
        #[cfg(test)]
        let status = 0u8;
        if status & TX_STATUS_DD != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    #[cfg(not(test))]
    let ok = unsafe {
        let desc = &raw const TX_DESCS.0[tdt];
        read_volatile(core::ptr::addr_of!((*desc).status)) & TX_STATUS_DD != 0
    };
    #[cfg(test)]
    let ok = true;
    ok
}

/// Try to receive a frame into `buf`. Returns the number of bytes received.
/// `timeout_iters` limits the poll loop.
#[cfg(not(test))]
pub fn try_receive(base: u64, buf: &mut [u8; 1514], timeout_iters: u64) -> Option<usize> {
    for _ in 0..timeout_iters {
        for idx in 0..NUM_RX_DESC {
            unsafe {
                let desc = &raw const RX_DESCS.0[idx];
                let status = read_volatile(core::ptr::addr_of!((*desc).status));
                if status & RX_STATUS_DD != 0 {
                    let len = read_volatile(core::ptr::addr_of!((*desc).length)) as usize;
                    let copy_len = if len > 1514 { 1514 } else { len };
                    core::ptr::copy_nonoverlapping(
                        core::ptr::addr_of!(RX_BUF) as *const u8,
                        buf.as_mut_ptr(),
                        copy_len,
                    );
                    let desc = &raw mut RX_DESCS.0[idx];
                    write_volatile(core::ptr::addr_of_mut!((*desc).status), 0u8);
                    reg_write32(base, REG_RDT, idx as u32);
                    return Some(copy_len);
                }
            }
        }
        core::hint::spin_loop();
    }
    None
}
