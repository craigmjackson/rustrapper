; Stage 2 entry stub
; Loaded by MBR at physical 0x1000 (14 sectors = 7168 bytes max).
; Enables A20, enters protected mode, copies the embedded Rust payload
; from the high portion of the loaded blob to 1 MB, and jumps there.
;
; After extending the MBR's load count, the first 512 bytes of this file
; are the entry stub itself.  Everything after the 512-byte mark is the
; Rust payload, copied verbatim to 0x100000.

%macro serial 1
    push dx
    push ax
    mov dx, 0x3F8
    mov al, %1
    out dx, al
    pop ax
    pop dx
%endmacro

[org 0x1000]
[bits 16]

start:
    cld
    serial 'R'

    mov [boot_drive], dl

    ; Stack just below our loaded location
    xor ax, ax
    mov ss, ax
    mov sp, 0x1000

    ; Set VGA text mode 80x25
    mov ax, 0x0003
    int 0x10
    serial 'V'

    ; Enable A20 gate (fast method)
    in al, 0x92
    or al, 2
    out 0x92, al
    serial 'A'

    ; Load GDT
    lgdt [gdtr]
    serial 'G'

    ; Enter protected mode
    mov eax, cr0
    or al, 1
    mov cr0, eax

    ; Far jump to 32-bit code
    jmp 0x08:pmode_start

[bits 32]
pmode_start:
    serial 'P'

    ; Flat data segments
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    ; Stack in low RAM (well below BIOS ROM at 0xF0000)
    mov esp, 0x00070000

    ; Copy Rust payload from 0x1200 → 0x100000
    mov esi, 0x1200
    mov edi, 0x100000
    mov ecx, RUST_PAYLOAD_BYTES
    rep movsb
    serial 'C'

    ; Zero BSS (static variables) right after the payload
    mov ecx, BSS_ZERO_SIZE
    xor eax, eax
    rep stosb
    serial 'Z'

    ; Pass boot_drive as argument and call Rust
    cli
    push dword [boot_drive]
    mov eax, 0x100000
    call eax
    serial 'E'  ; Should never reach here

; ── Data ──────────────────────────────────────────────────────────────
align 4

boot_drive:  dd 0

; RUST_PAYLOAD_BYTES and BSS_ZERO_SIZE are computed at assembly time.
; BSS_ZERO_SIZE covers BSS for descriptor rings, packet buffers, and statics.
RUST_PAYLOAD_BYTES equ __payload_end - __payload_start
BSS_ZERO_SIZE     equ 0x2000

gdt:
    dq 0                    ; Null
    dw 0xFFFF, 0x0000, 0x9A00, 0x00CF  ; Code (base 0, limit 4G, 32-bit)
    dw 0xFFFF, 0x0000, 0x9200, 0x00CF  ; Data (base 0, limit 4G, writable)
gdt_end:

gdtr:
    dw gdt_end - gdt - 1
    dd gdt

; Pad to 512 bytes so the Rust payload starts at a known offset (0x1200)
times 512 - ($ - $$) db 0

; ── Rust payload embedded here ────────────────────────────────────────
__payload_start:
    incbin "../bin/rust_payload.bin"
__payload_end:
