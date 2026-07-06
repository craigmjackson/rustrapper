#include "print.h"

static putc_fn g_putc __attribute__((section(".data")));

void print_init(putc_fn fn)
{
    g_putc = fn;
}

void putc(char c)
{
    if (g_putc)
        g_putc(c);
}

void puts(const char *s)
{
    while (*s)
        putc(*s++);
}

void print_hex(unsigned long long val, int nibbles)
{
    int started = 0;
    for (int i = nibbles - 1; i >= 0; i--)
    {
        int digit = (int)((val >> (i * 4)) & 0xF);
        if (digit == 0 && !started && i > 0)
            continue;
        started = 1;
        putc(digit < 10 ? '0' + digit : 'A' + digit - 10);
    }
    if (!started)
        putc('0');
}

void print_dec(unsigned long long val)
{
    char buf[20];
    int i = 0;
    if (val == 0)
    {
        putc('0');
        return;
    }
    while (val)
    {
        buf[i++] = '0' + (unsigned int)(val % 10);
        val /= 10;
    }
    while (i > 0)
        putc(buf[--i]);
}
