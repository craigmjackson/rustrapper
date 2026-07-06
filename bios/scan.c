#include "print.h"
#include "scan.h"

void scan_devices(void)
{
    puts("Scanning for devices...\r\n");
    struct device_info info;
    int found = 0;
    for (int i = 0; i < MAX_DEVICES; i++)
    {
        info.present = 0;
        info.block_count = 0;
        info.block_size = 512;
        info.removable = 0;
        info.description = 0;
        if (!detect_device(i, &info))
            continue;
        if (!info.present)
            continue;
        found++;
        puts("  Device ");
        print_dec(i);
        puts(": ");
        char c = info.removable ? 'R' : 'F';
        putc(c);
        puts(" ");
        if (info.description)
            puts(info.description);
        puts(" (");
        print_hex(info.index, 2);
        puts(") - ");
        print_dec(info.block_count);
        puts(" x ");
        print_dec(info.block_size);
        puts(" byte sectors\r\n");
    }
    puts("Found ");
    print_dec(found);
    puts(" device(s)\r\n");
}
