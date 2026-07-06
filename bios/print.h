#ifndef COMMON_PRINT_H
#define COMMON_PRINT_H

typedef void (*putc_fn)(char);

void print_init(putc_fn fn);
void putc(char c);
void puts(const char *s);
void print_hex(unsigned long long val, int nibbles);
void print_hex32(unsigned int val, int nibbles);
void print_dec(unsigned long long val);

#endif
