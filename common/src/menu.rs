pub enum MenuAction {
    StorageScan,
    NetworkBoot,
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
            _ => puts("Invalid choice, try again.\n"),
        }
    }
}
