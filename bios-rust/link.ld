OUTPUT_FORMAT(elf32-i386)
ENTRY(_start)

/* Rust payload is loaded at 1 MB by the entry stub */
BASE_ADDRESS = 0x100000;

SECTIONS
{
    . = BASE_ADDRESS;

    .text : ALIGN(4) {
        *(.text._start)
        *(.text*)
    }

    .rodata : ALIGN(4) {
        *(.rodata*)
    }

    .data : ALIGN(4) {
        *(.data*)
    }

    .bss : ALIGN(4) {
        __bss_start = .;
        *(COMMON)
        *(.bss*)
        . = ALIGN(4);
        __bss_end = .;
    }

    /DISCARD/ : {
        *(.comment*)
        *(.note*)
        *(.eh_frame*)
        *(.got*)
    }
}
