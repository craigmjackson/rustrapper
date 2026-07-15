; Minimal MBR - loads stage-2 from disk and jumps to it
; Note: "MZ" at offset 0 executes as `dec bp; pop dx` which
; corrupts DL. So we try known drive numbers explicitly.
[org 0x7C00]
[bits 16]

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
    mov si, dap
    mov ah, 0x42
    int 0x13
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
    jmp 0x0100:0x0000

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

; Disk Address Packet for extended read
dap:
    db 0x10        ; size
    db 0x00        ; reserved
    dw 32          ; sectors to read (LBA 1-32 = bytes 0x200-0x40FF)
    dw 0x1000      ; buffer offset
    dw 0x0000      ; buffer segment
    dq 1           ; start LBA

times 510-($-$$) db 0
dw 0xAA55
