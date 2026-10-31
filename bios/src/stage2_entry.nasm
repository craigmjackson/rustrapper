; Stage 2 entry stub
; Loaded by MBR at physical 0x8000 (256 sectors max).
; Captures the BIOS E820 memory map (real mode only), enables A20, enters
; protected mode, copies the embedded Rust payload from the high portion of
; the loaded blob to 1 MB, and jumps there.
;
; The first 512 bytes of this file are the entry stub itself.
; Everything after the 512-byte mark is the Rust payload, copied
; verbatim to 0x100000.

; Physical location of the captured E820 map (24-byte entries) and the
; maximum number of entries. Lives in free low memory: above the BIOS data
; area (0x400-0x4FF) and below the MBR/stage2 at 0x7C00/0x8000.
E820_ADDR   equ 0x500
E820_MAX    equ 64
E820_ENTRY  equ 24

%macro serial 1
    push dx
    push ax
    mov dx, 0x3F8
    mov al, %1
    out dx, al
    pop ax
    pop dx
%endmacro

[org 0x8000]
[bits 16]

start:
    cld
    serial 'R'

    mov [boot_drive], dl

    ; Stack just below our loaded location (0x8000), above the MBR
    xor ax, ax
    mov ss, ax
    mov sp, 0x7C00

    ; Capture the BIOS memory map now: INT 15h is not callable after the
    ; protected-mode switch (the stage2 runs with interrupts disabled and no
    ; IDT).
    call e820_capture
    serial 'M'

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

    ; Copy Rust payload from 0x8200 → 0x100000
    mov esi, 0x8200
    mov edi, 0x100000
    mov ecx, RUST_PAYLOAD_BYTES
    rep movsb
    serial 'C'

    ; Zero BSS (static variables) right after the payload
    mov ecx, BSS_ZERO_SIZE
    xor eax, eax
    rep stosb
    serial 'Z'

    ; Call Rust: _start(boot_drive, e820_addr, e820_count, bootdisk_fn) —
    ; cdecl, so the rightmost argument is pushed first.
    cli
    push dword boot_hard_disk
    push dword [e820_count]
    mov eax, E820_ADDR
    push eax
    push dword [boot_drive]
    mov eax, 0x100000
    call eax
    serial 'E'  ; Should never reach here

; ── Boot from hard disk: chainload MBR at 0x7C00 ────────────────
boot_hard_disk:
    [bits 32]
    cli
    jmp 0x18:boot_hard_disk_16
    [bits 16]
boot_hard_disk_16:
    lidt [real_idt]
    o32 mov eax, cr0
    and al, 0xFE
    o32 mov cr0, eax
    jmp 0x0000:boot_hard_disk_real
boot_hard_disk_real:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00
    mov dl, 0x80
    ; Read MBR of drive 0x80
    mov ah, 0x02
    mov al, 1
    mov ch, 0
    mov cl, 1
    mov dh, 0
    mov bx, 0x7C00
    int 0x13
    jc .fail
    ; If rustrapper's own MBR, find active partition
    cmp word [0x7C00], 0x5A4D
    jne .boot_mbr
    mov si, 0x7C00 + 0x1BE
    mov cx, 4
.part_loop:
    cmp byte [si], 0x80
    je .found
    add si, 16
    loop .part_loop
    jmp .fail
.found:
    ; Use CHS from partition table entry (offset 1-3)
    mov dh, [si+1]
    mov cx, [si+2]
    mov ah, 0x02
    mov al, 1
    mov bx, 0x7C00
    int 0x13
    jc .fail
.boot_mbr:
    cmp word [0x7C00+510], 0xAA55
    jne .fail
    sti
    jmp 0x0000:0x7C00
.fail:
    hlt
    jmp .fail

[bits 16]
; ── E820 memory map capture (real mode) ───────────────────────────────
; Fills E820_ADDR with up to E820_MAX 24-byte entries and stores the count
; in [e820_count]. Calls INT 15h/AX=E820 repeatedly (EBX = continuation).
; Entries shorter than 24 bytes (ECX=20 on old BIOSes) are fine: the buffer
; is pre-zeroed and we always advance by 24. Preserves all registers used.
e820_capture:
    push ax
    push bx
    push cx
    push dx
    push si
    push di
    push ds
    push es

    xor ax, ax
    mov ds, ax
    mov es, ax

    ; Zero the buffer (so short entries keep a zero ACPI field)
    mov di, E820_ADDR
    mov cx, (E820_MAX * E820_ENTRY) / 2
    xor ax, ax
    rep stosw

    mov dword [e820_count], 0
    mov word [e820_di], E820_ADDR
    xor ebx, ebx                    ; EBX = 0: start of the map
.next:
    mov eax, 0xE820
    mov edx, 0x534D4150             ; 'SMAP'
    mov ecx, E820_ENTRY
    mov di, [e820_di]
    int 0x15
    jc .done                        ; unsupported / error: keep what we have
    cmp eax, 0x534D4150
    jne .done                       ; not an SMAP-compatible BIOS
    cmp dword [e820_count], E820_MAX
    jae .done                       ; buffer full
    add word [e820_di], E820_ENTRY
    inc dword [e820_count]
    test ebx, ebx
    jnz .next

.done:
    pop es
    pop ds
    pop di
    pop si
    pop dx
    pop cx
    pop bx
    pop ax
    ret

; ── Data ──────────────────────────────────────────────────────────────
align 4

boot_drive:  dd 0
e820_count:  dd 0
e820_di:     dw 0

; RUST_PAYLOAD_BYTES and BSS_ZERO_SIZE are computed at assembly time.
; BSS_ZERO_SIZE covers BSS for descriptor rings, packet buffers, and statics.
RUST_PAYLOAD_BYTES equ __payload_end - __payload_start
BSS_ZERO_SIZE     equ 0x5000

gdt:
    dq 0                    ; Null
    dw 0xFFFF, 0x0000, 0x9A00, 0x00CF  ; 0x08 Code (base 0, limit 4G, 32-bit)
    dw 0xFFFF, 0x0000, 0x9200, 0x00CF  ; 0x10 Data (base 0, limit 4G, writable)
    dw 0xFFFF, 0x0000, 0x9A00, 0x0000  ; 0x18 Code (base 0, limit 64K, 16-bit)
gdt_end:

real_idt:
    dw 0x03FF
    dd 0x00000000

gdtr:
    dw gdt_end - gdt - 1
    dd gdt

; Pad to 512 bytes so the Rust payload starts at a known offset (0x8200)
times 512 - ($ - $$) db 0

; ── Rust payload embedded here ────────────────────────────────────────
__payload_start:
    incbin "../../bin/rust_payload.bin"
__payload_end:
