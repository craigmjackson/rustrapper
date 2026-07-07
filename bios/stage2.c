#include "print.h"
#include "scan.h"
#include "pxe.h"

__asm__(
    ".globl _start\n"
    "_start:\n"
    "  cli\n"
    "  mov %cs, %ax\n"
    "  mov %ax, %ds\n"
    "  mov %ax, %ss\n"
    "  xor %eax, %eax\n"
    "  mov $0xFFFF, %ax\n"
    "  mov %ax, %sp\n"
    "  call stage2_main\n"
    "  cli\n"
    "  hlt\n"
);

static unsigned char dpb_buf[32] __attribute__((section(".data"))) = {0};

static void serial_out(char c)
{
    __asm__ volatile (
        "mov $0x3FD, %%dx\n"
        "1:\n"
        "in %%dx, %%al\n"
        "test $0x20, %%al\n"
        "jz 1b\n"
        "mov $0x3F8, %%dx\n"
        "mov %0, %%al\n"
        "out %%al, %%dx\n"
        : : "r"(c) : "dx", "al", "cc"
    );
}

static void serial_init(void)
{
    __asm__ volatile (
        "mov $0x3FB, %%dx\n"
        "mov $0x80, %%al\n"
        "out %%al, %%dx\n"
        "mov $0x3F8, %%dx\n"
        "mov $0x01, %%al\n"
        "out %%al, %%dx\n"
        "mov $0x3F9, %%dx\n"
        "xor %%al, %%al\n"
        "out %%al, %%dx\n"
        "mov $0x3FB, %%dx\n"
        "mov $0x03, %%al\n"
        "out %%al, %%dx\n"
        "mov $0x3FA, %%dx\n"
        "mov $0xC7, %%al\n"
        "out %%al, %%dx\n"
        : : : "dx", "al"
    );
}

int detect_device(int index, struct device_info *info)
{
    unsigned char drive;
    if (index < 8)
        drive = 0x80 + index;
    else if (index < 12)
        drive = 0x00 + (index - 8);
    else
        return 0;

    info->index = drive;
    info->present = 0;

    {
        unsigned short ax = 0x1500;
        unsigned short dx = drive;
        unsigned char cf;
        __asm__ volatile (
            "int $0x13\n"
            "setc %0\n"
            : "=m"(cf), "+a"(ax), "+d"(dx)
            : : "cc", "bx", "cx", "si", "di"
        );
        if (cf) return 0;
        unsigned char type = (ax >> 8) & 0xFF;
        if (type == 0) return 0;
        info->present = 1;
        info->removable = (type != 3);
        info->description = (type == 3) ? "Hard Drive" :
                            (type == 1) ? "Floppy" :
                            (type == 2) ? "Floppy" : "Unknown";
    }

    {
        unsigned short ax = 0x4800;
        unsigned short dx = drive;
        unsigned short si = (unsigned short)(unsigned long)dpb_buf;
        unsigned char cf;
        dpb_buf[0] = 0x1E;
        dpb_buf[1] = 0x00;
        __asm__ volatile (
            "int $0x13\n"
            "setc %0\n"
            : "=m"(cf), "+a"(ax), "+d"(dx), "+S"(si)
            : : "cc", "bx", "cx", "di", "memory"
        );
        if (!cf) {
            info->block_size = *(unsigned short *)(dpb_buf + 24);
            info->block_count = *(unsigned long long *)(dpb_buf + 16);
        }
    }

    return 1;
}

void stage2_main(void)
{
    serial_init();
    print_init(serial_out);
    puts("\r\nStage 2 BIOS\r\n");
    pxe_scan();
    puts("\r\n");
    scan_devices();
    puts("Halting.\r\n");
    for (;;)
        __asm__ volatile ("hlt");
}
