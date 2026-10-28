#define _POSIX_C_SOURCE 200809L
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdatomic.h>
#include <time.h>
#ifndef ABI
#define ABI 1
#endif
static _Atomic int active;
struct channel { unsigned char bytes[65536]; uint32_t size; };
uint32_t sim_t2u_abi_version(void) { return ABI; }
int test_active(void) { return atomic_load(&active); }
int sim_t2u_open(const char *server, uint16_t port, const char *key,
    const char *serial, const char *password, const char *ip, uint16_t remote,
    uint32_t timeout, void **out, void *report) {
    (void)port; (void)key; (void)password; (void)ip; (void)remote; (void)timeout; (void)report;
    if (strcmp(serial, "TEST-SERIAL")) return -1;
    if (!strcmp(server, "slow")) {
        struct timespec delay = {0, 200000000}; nanosleep(&delay, NULL);
    }
    *out = calloc(1, sizeof(struct channel));
    if (!*out) return -3;
    atomic_fetch_add(&active, 1);
    return 0;
}
int sim_t2u_write(struct channel *c, const void *data, uint32_t size, uint32_t timeout) {
    (void)timeout;
    if (c->size + size > sizeof c->bytes) return -3;
    memcpy(c->bytes + c->size, data, size); c->size += size; return (int)size;
}
int sim_t2u_read(struct channel *c, void *data, uint32_t size, uint32_t timeout) {
    if (!c->size) {
        struct timespec delay = {0, timeout * 1000000}; nanosleep(&delay, NULL); return -4;
    }
    if (size > c->size) size = c->size;
    memcpy(data, c->bytes, size); c->size -= size;
    memmove(c->bytes, c->bytes + size, c->size); return (int)size;
}
void sim_t2u_close(struct channel *c) { free(c); atomic_fetch_sub(&active, 1); }
