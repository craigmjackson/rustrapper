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


// ─── Direct MMIO e1000 driver (no UEFI protocols) ───
//
// Thin wrapper around `common::e1000` and `common::dhcp`. The UEFI target
// is the only one that needs a UEFI-specific output sink (con_out) for
// printing the DHCP result, so we keep the init/send/recv/dhcp logic in
// common and only add the UEFI output adapter here.

struct DirectMmioE1000 {
    base: u64,
}

impl DirectMmioE1000 {
    fn new(bar0: u64) -> Self {
        Self { base: bar0 }
    }

    fn read_mac(&self) -> [u8; 6] {
        common::e1000::read_mac(self.base)
    }

    fn init(&self) -> bool {
        common::e1000::init(self.base).is_some()
    }

    fn send(&self, data: &[u8]) -> bool {
        common::e1000::send(self.base, data)
    }

    fn receive_into(&self, buf: &mut [u8; 1514]) -> Option<usize> {
        common::e1000::try_receive(self.base, buf, 200_000_000)
    }

    fn dhcp_run(&self) -> Option<common::dhcp::DhcpConfig> {
        let mac = self.read_mac();
        let xid = 0x12345678;
        let mut frame = [0u8; 1514];
        let dhcp_payload = common::dhcp::build_discover(xid, &mac);
        let frame_len = common::dhcp::build_eth_ip_udp(&mac, &dhcp_payload, 300, &mut frame);
        if !self.send(&frame[..frame_len]) {
            return None;
        }
        if let Some(len) = self.receive_into(&mut frame) {
            return common::dhcp::parse_response(&frame, len, xid, &mac);
        }
        None
    }
}


fn scan_pci_direct(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
) -> Option<common::dhcp::DhcpConfig> {
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

                            dns_resolve_e1000(con_out, e1000.base, &mac, &cfg);
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
) -> Option<common::dhcp::DhcpConfig> {
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

fn e1000_init_and_dhcp(con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL, bar0: u64) -> Option<common::dhcp::DhcpConfig> {
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

    dns_resolve_e1000(con_out, bar0 as u64, &mac, &config);

    Some(config)
}

fn try_loaded_image_path(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    gbs: *mut c_void,
    image_handle: EFI_HANDLE,
) -> Option<common::dhcp::DhcpConfig> {
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
) -> Option<common::dhcp::DhcpConfig> {
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
) -> Option<common::dhcp::DhcpConfig> {
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

// ─── SNP-based network scan ───

fn dhcp_run(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    snp: &EFI_SIMPLE_NETWORK_PROTOCOL,
    mac: &[u8; 6],
) -> Option<common::dhcp::DhcpConfig> {
    let xid: u32 = 0x12345678;

    for _ in 0..50000 {
        let mode = unsafe { &*snp.mode };
        if mode.media_present != 0 { break; }
        for _ in 0..100 { core::hint::spin_loop(); }
    }

    w16(con_out, "Sending DHCPDISCOVER...");
    let discover = common::dhcp::build_discover(xid, mac);
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
    let frame_len = common::dhcp::build_eth_ip_udp(mac, dhcp_payload, dhcp_len, &mut frame);

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
) -> Option<common::dhcp::DhcpConfig> {
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

    common::dhcp::parse_response(&frame, buffer_size as usize, xid, mac)
}

// ─── SNP-based ARP and DNS (for x86_64 UEFI with full SNP support) ───

/// Restart the SNP adapter to recycle TX/RX buffers. The ARM64 virtio SNP
/// driver doesn't support `Initialize` (returns `EFI_UNSUPPORTED`), so
/// TX buffers are never recycled — the ring fills up after a few transmits.
/// Calling `Stop` + `Start` + `Initialize` fully restarts the driver.
fn restart_snp(snp: &EFI_SIMPLE_NETWORK_PROTOCOL) {
    unsafe {
        (snp.shutdown)(snp as *const _ as *mut _);
        (snp.stop)(snp as *const _ as *mut _);
        (snp.start)(snp as *const _ as *mut _);
        (snp.initialize)(snp as *const _ as *mut _, 0, 0);
    }
}

fn send_raw_snp(snp: &EFI_SIMPLE_NETWORK_PROTOCOL, data: &[u8]) -> bool {
    let mut frame = [0u8; 1514];
    let len = data.len().min(1514);
    frame[..len].copy_from_slice(&data[..len]);
    let st = unsafe {
        (snp.transmit)(
            snp as *const _ as *mut _,
            0,
            len as UINTN,
            frame.as_mut_ptr() as *mut c_void,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    st == EFI_SUCCESS
}

fn try_receive_raw_snp(snp: &EFI_SIMPLE_NETWORK_PROTOCOL, buf: &mut [u8; 1514]) -> Option<usize> {
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
            buf.as_mut_ptr() as *mut c_void,
            &mut src_addr,
            &mut dst_addr,
            &mut protocol,
        )
    };

    if st != EFI_SUCCESS {
        return None;
    }
    Some(buffer_size as usize)
}

fn arp_resolve_snp(
    snp: &EFI_SIMPLE_NETWORK_PROTOCOL,
    mac: &[u8; 6],
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
) -> Option<[u8; 6]> {
    let mut frame = [0u8; 1514];
    let len = common::arp::build_request(mac, src_ip, dst_ip, &mut frame);
    if !send_raw_snp(snp, &frame[..len]) {
        return None;
    }
    let mut buf = [0u8; 1514];
    for _ in 0..500000 {
        if let Some(len) = try_receive_raw_snp(snp, &mut buf) {
            if let Some(m) = common::arp::parse_reply(&buf, len, dst_ip) {
                return Some(m);
            }
        }
        for _ in 0..20 { core::hint::spin_loop(); }
    }
    None
}

fn dns_lookup_snp(
    snp: &EFI_SIMPLE_NETWORK_PROTOCOL,
    mac: &[u8; 6],
    src_ip: [u8; 4],
    dst_mac: &[u8; 6],
    dst_ip: [u8; 4],
    name: &str,
) -> Option<[u8; 4]> {
    let id: u16 = 0x1234;
    let src_port: u16 = 0x3039;
    let mut query = [0u8; 256];
    let query_len = common::dns::build_query(id, name, &mut query);
    if query_len == 0 {
        return None;
    }
    let mut frame = [0u8; 1514];
    let frame_len = common::dns::build_frame(
        mac, src_ip, dst_mac, dst_ip, src_port, &query, query_len, &mut frame,
    );
    restart_snp(snp);
    if !send_raw_snp(snp, &frame[..frame_len]) {
        return None;
    }
    let mut buf = [0u8; 1514];
    for _ in 0..500000 {
        if let Some(len) = try_receive_raw_snp(snp, &mut buf) {
            if let Some(ip) = common::dns::parse_response(&buf, len, id) {
                return Some(ip);
            }
        }
        for _ in 0..20 { core::hint::spin_loop(); }
    }
    None
}

/// DNS resolve + print for the SNP path. Uses con_out for output.
fn dns_resolve_snp(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    snp: &EFI_SIMPLE_NETWORK_PROTOCOL,
    mac: &[u8; 6],
    cfg: &common::dhcp::DhcpConfig,
) {
    if cfg.dns == [0u8; 4] {
        w16(con_out, "  DNS: (none)\r\n");
        return;
    }
    w16(con_out, "  DNS server: ");
    print_ip(con_out, &cfg.dns);
    w16(con_out, "\r\n");

    let next_hop = if common::netio::same_subnet(cfg.yiaddr, cfg.dns, cfg.subnet) {
        cfg.dns
    } else if cfg.gateway != [0u8; 4] {
        cfg.gateway
    } else {
        w16(con_out, "  Cannot resolve: no gateway\r\n");
        return;
    };

    w16(con_out, "  ARP ");
    print_ip(con_out, &next_hop);
    w16(con_out, "...");
    match arp_resolve_snp(snp, mac, cfg.yiaddr, next_hop) {
        Some(dst_mac) => {
            w16(con_out, "OK\r\n");
            w16(con_out, "  DNS: google.com...");
            let result = dns_lookup_snp(snp, mac, cfg.yiaddr, &dst_mac, cfg.dns, "google.com");
            match result {
                Some(ip) => {
                    w16(con_out, "OK\r\n");
                    w16(con_out, "  google.com: ");
                    print_ip(con_out, &ip);
                    w16(con_out, "\r\n");
                }
                None => w16(con_out, "FAILED\r\n"),
            }
        }
        None => w16(con_out, "FAILED\r\n"),
    }
}

/// DNS resolve + print for the direct e1000 path. Uses con_out for output.
fn dns_resolve_e1000(
    con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL,
    base: u64,
    mac: &[u8; 6],
    cfg: &common::dhcp::DhcpConfig,
) {
    if cfg.dns == [0u8; 4] {
        w16(con_out, "  DNS: (none)\r\n");
        return;
    }
    w16(con_out, "  DNS server: ");
    print_ip(con_out, &cfg.dns);
    w16(con_out, "\r\n");

    let next_hop = if common::netio::same_subnet(cfg.yiaddr, cfg.dns, cfg.subnet) {
        cfg.dns
    } else if cfg.gateway != [0u8; 4] {
        cfg.gateway
    } else {
        w16(con_out, "  Cannot resolve: no gateway\r\n");
        return;
    };

    w16(con_out, "  ARP ");
    print_ip(con_out, &next_hop);
    w16(con_out, "...");
    match common::netio::arp_resolve_e1000(base, mac, cfg.yiaddr, next_hop) {
        Some(dst_mac) => {
            w16(con_out, "OK\r\n");
            w16(con_out, "  DNS: google.com...");
            match common::netio::dns_lookup_e1000(base, mac, cfg.yiaddr, &dst_mac, cfg.dns, "google.com") {
                Some(ip) => {
                    w16(con_out, "OK\r\n");
                    w16(con_out, "  google.com: ");
                    print_ip(con_out, &ip);
                    w16(con_out, "\r\n");
                }
                None => w16(con_out, "FAILED\r\n"),
            }
        }
        None => w16(con_out, "FAILED\r\n"),
    }
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

                    dns_resolve_snp(con_out, snp, &mac, &cfg);
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

