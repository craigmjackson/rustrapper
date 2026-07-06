SECTIONS
{
    . = 0x40200000;
    .text : { *(.text.boot) *(.text) }
    .rodata : { *(.rodata) }
    .data : { *(.data) }
    .bss : {
        _bss_start = .;
        *(.bss)
        *(COMMON)
        _bss_end = .;
    }
    . = ALIGN(16);
    . += 0x10000;
    _stack_top = .;
}
