#ifdef __i386__

unsigned long long __udivdi3(unsigned long long a, unsigned long long b)
{
    unsigned int *ap = (unsigned int *)&a;
    unsigned int *bp = (unsigned int *)&b;

    if (bp[1] == 0 && bp[0] == 0) return 0;
    if (bp[1] == 0 && ap[1] < bp[0])
    {
        unsigned int q;
        __asm__ ("divl %3" : "=a"(q) : "a"(ap[0]), "d"(ap[1]), "r"(bp[0]));
        unsigned long long r;
        ((unsigned int *)&r)[0] = q;
        ((unsigned int *)&r)[1] = 0;
        return r;
    }

    unsigned long long q = 0, r = 0;
    for (int i = 63; i >= 0; i--)
    {
        unsigned int *rp = (unsigned int *)&r;
        unsigned int lo = rp[0], hi = rp[1];
        rp[0] = lo << 1;
        rp[1] = (hi << 1) | (lo >> 31);

        int bit = (i >= 32) ? ((ap[1] >> (i - 32)) & 1) : ((ap[0] >> i) & 1);
        rp[0] |= bit;

        if (rp[1] > bp[1] || (rp[1] == bp[1] && rp[0] >= bp[0]))
        {
            unsigned int nl = rp[0] - bp[0];
            rp[1] = rp[1] - bp[1] - (nl > rp[0] ? 1 : 0);
            rp[0] = nl;

            unsigned int *qp = (unsigned int *)&q;
            if (i >= 32) qp[1] |= (1u << (i - 32));
            else         qp[0] |= (1u << i);
        }
    }
    return q;
}

unsigned long long __umoddi3(unsigned long long a, unsigned long long b)
{
    unsigned int *ap = (unsigned int *)&a;
    unsigned int *bp = (unsigned int *)&b;

    if (bp[1] == 0 && bp[0] == 0) return 0;
    if (bp[1] == 0 && ap[1] < bp[0])
    {
        unsigned int r;
        __asm__ ("divl %4" : "=a"(ap[0]), "=d"(r) : "a"(ap[0]), "d"(ap[1]), "r"(bp[0]));
        unsigned long long res;
        ((unsigned int *)&res)[0] = r;
        ((unsigned int *)&res)[1] = 0;
        return res;
    }

    unsigned long long r = 0;
    unsigned int *bp2 = (unsigned int *)&b;
    unsigned int bl = bp2[0], bh = bp2[1];
    for (int i = 63; i >= 0; i--)
    {
        unsigned int *rp = (unsigned int *)&r;
        unsigned int lo = rp[0], hi = rp[1];
        rp[0] = lo << 1;
        rp[1] = (hi << 1) | (lo >> 31);

        int bit = (i >= 32) ? ((ap[1] >> (i - 32)) & 1) : ((ap[0] >> i) & 1);
        rp[0] |= bit;

        unsigned int rl = rp[0], rh = rp[1];
        if (rh > bh || (rh == bh && rl >= bl))
        {
            unsigned int nl = rl - bl;
            rp[1] = rh - bh - (nl > rl ? 1 : 0);
            rp[0] = nl;
        }
    }
    return r;
}

unsigned long long __udivmoddi4(unsigned long long a, unsigned long long b, unsigned long long *rem)
{
    *rem = __umoddi3(a, b);
    return __udivdi3(a, b);
}

#endif
