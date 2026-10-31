pub enum MenuAction {
    StorageScan,
    NetworkBoot,
    LuaShell,
    HardDiskBoot,
}

pub fn show_menu(
    puts: fn(&str),
    putc: fn(u8),
    get_key: fn() -> Option<u8>,
) -> MenuAction {
    loop {
        puts("\nMenu:\n");
        puts("  [1] List storage devices\n");
        puts("  [2] Boot from network\n");
        puts("  [3] Lua Shell\n");
        puts("  [4] Boot from hard disk\n");
        puts("Choose: ");
        let key = loop {
            if let Some(k) = get_key() {
                break k;
            }
        };
        putc(key);
        puts("\n\n");
        match key {
            b'1' => return MenuAction::StorageScan,
            b'2' => return MenuAction::NetworkBoot,
            b'3' => return MenuAction::LuaShell,
            b'4' => return MenuAction::HardDiskBoot,
            _ => puts("Invalid choice, try again.\n"),
        }
    }
}
