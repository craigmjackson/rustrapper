; Minimal MBR - loads stage-2 from disk and jumps to it
; Note: "MZ" at offset 0 executes as `dec bp; pop dx` which
; corrupts DL. So we try known drive numbers explicitly.
[org 0x7C00]
[bits 16]

TOTAL_SECTORS equ 256
MAX_CHUNK     equ 127

; Offset 0x00: "MZ" signature for PE compatibility
dw 0x5A4D

; Offset 0x02: real boot code starts here (DL is already corrupted!)
start:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7C00

    ; Try hard disk (0x80) first, then floppy (0x00)
    mov byte [drive_num], 0x80
.try:
    mov dl, [drive_num]
    call load_stage2
    jnc .loaded

    cmp byte [drive_num], 0x80
    jne .fail
    mov byte [drive_num], 0x00
    jmp .try

.fail:
    mov si, msg_err
    call puts
.halt:
    hlt
    jmp .halt

.loaded:
    mov dl, [drive_num]
    jmp 0x0800:0x0000

; Load TOTAL_SECTORS sectors starting at LBA 1 into 0x0000:0x8000, in chunks
; of at most MAX_CHUNK sectors: SeaBIOS (and some real BIOSes) reject a single
; extended read with count > 127. Each 127-sector chunk advances the buffer
; segment by 127*32 = 0xFE0, so the offset stays 0x8000.
; Returns with CF=0 on success, CF=1 on error.
load_stage2:
    push ax
    push bx
    push cx
    push dx
    push si
    mov cx, TOTAL_SECTORS
    mov word [dap_seg], 0x0000
    mov word [dap_lba], 1
    mov word [dap_lba + 2], 0
    mov word [dap_lba + 4], 0
    mov word [dap_lba + 6], 0
.next:
    mov bx, cx
    cmp bx, MAX_CHUNK
    jbe .count_ok
    mov bx, MAX_CHUNK
.count_ok:
    mov [dap_count], bx
    mov dl, [drive_num]
    mov si, dap
    mov ah, 0x42
    int 0x13
    jc .done
    ; lba += bx
    add [dap_lba], bx
    adc word [dap_lba + 2], 0
    adc word [dap_lba + 4], 0
    adc word [dap_lba + 6], 0
    ; buffer segment += bx * 32 (512 bytes per sector)
    push dx
    mov ax, bx
    mov dx, 32
    mul dx
    add [dap_seg], ax
    pop dx
    sub cx, bx
    jnz .next
    clc
.done:
    pop si
    pop dx
    pop cx
    pop bx
    pop ax
    ret

; Print null-terminated string at DS:SI via INT 10h
puts:
    push ax
    push si
.l:
    lodsb
    cmp al, 0
    je .x
    mov ah, 0x0E
    int 0x10
    jmp .l
.x:
    pop si
    pop ax
    ret

; Variables
drive_num: db 0

; Strings
msg_err: db 'Boot error', 0x0D, 0x0A, 0

; Disk Address Packet for extended reads. count/segment/LBA are filled in per
; chunk. Buffer runs 0x8000..0x28000 (256 sectors), above the MBR at 0x7C00
; and below the EBDA at ~0x9FC00.
dap:
    db 0x10        ; size
    db 0x00        ; reserved
dap_count: dw 0
dap_off:   dw 0x8000
dap_seg:   dw 0x0000
dap_lba:   dq 1

times 510-($-$$) db 0
dw 0xAA55
