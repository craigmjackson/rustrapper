#ifndef COMMON_SCAN_H
#define COMMON_SCAN_H

#define MAX_DEVICES 32

struct device_info {
    unsigned char  index;
    unsigned char  present;
    unsigned char  removable;
    unsigned long long block_count;
    unsigned short block_size;
    const char    *description;
};

int detect_device(int index, struct device_info *info);
void scan_devices(void);

#endif
