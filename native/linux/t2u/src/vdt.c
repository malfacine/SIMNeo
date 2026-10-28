#define _POSIX_C_SOURCE 200809L
#include "vdt.h"
#include "protocol.h"
#include <arpa/inet.h>
#include <errno.h>
#include <poll.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/random.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

/* A WAN video stream needs more than 32 packets in flight at high playback
 * rates. Keep receive storage bounded independently of the public write cap. */
#define STREAM_CAP (512 * 1024)
#define WRITE_CAP 65536
#define CHUNK 1024
struct sim_t2u_channel {
    int fd, ready, closed;
    struct sockaddr_in peer;
    char uuid[100];
    uint32_t connection_id, tx_sequence, rx_sequence, rx_seen, acknowledged, peer_sequence;
    uint64_t next_heartbeat;
    struct sockaddr_in coordinator;
    char service_key[32];
    uint32_t service_interval;
    uint64_t next_service;
    unsigned char stream[STREAM_CAP];
    size_t used;
    unsigned char record_header[8];
    size_t header_used;
    uint32_t record_id, remaining;
};
static uint64_t now_ms(void) {
    struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000 + (uint64_t)t.tv_nsec / 1000000;
}
static void be16(unsigned char *p, uint16_t value) {
    p[0] = (unsigned char)(value >> 8); p[1] = (unsigned char)value;
}
static void be32(unsigned char *p, uint32_t value) {
    for (unsigned i = 0; i < 4; ++i) p[i] = (unsigned char)(value >> (24 - 8 * i));
}
static uint16_t u16(const unsigned char *p) { return (uint16_t)(p[0] << 8 | p[1]); }
static uint32_t u32(const unsigned char *p) {
    return (uint32_t)p[0] << 24 | (uint32_t)p[1] << 16 | (uint32_t)p[2] << 8 | p[3];
}
static void header(unsigned char *p, unsigned type) {
    memset(p, 0, 16); be16(p, 0x1201); be16(p + 2, (uint16_t)(0x1000 | type));
}
static int emit(struct sim_t2u_channel *c, const unsigned char *p, size_t n) {
    return sendto(c->fd, p, n, 0, (struct sockaddr *)&c->peer, sizeof c->peer) == (ssize_t)n
        ? 0 : SIM_T2U_IO;
}
int vdt_configure_service(struct sim_t2u_channel *c, const struct sockaddr_in *coordinator,
    const char *key, uint32_t interval_ms) {
    if (!c || !coordinator || !key || !*key || strlen(key) >= 32 ||
        interval_ms < 100 || interval_ms > 60000) return SIM_T2U_INPUT;
    c->coordinator = *coordinator;
    memcpy(c->service_key, key, strlen(key) + 1);
    c->service_interval = interval_ms;
    c->next_service = now_ms() + interval_ms;
    return 0;
}
static int service_heartbeat(struct sim_t2u_channel *c) {
    unsigned char p[160] = {0};
    struct timeval t; gettimeofday(&t, NULL);
    write_le32(p, 0x03f42012); write_le32(p + 4, (uint32_t)t.tv_sec);
    if (getrandom(p + 8, 8, 0) != 8) return SIM_T2U_IO;
    memcpy(p + 16, c->service_key, strlen(c->service_key));
    tea_encrypt(p + 16, 32, p);
    memcpy(p + 48, c->uuid, strlen(c->uuid));
    write_le32(p + 148, 121); write_le32(p + 152, 5); write_le32(p + 156, 3);
    int code = sendto(c->fd, p, sizeof p, 0, (struct sockaddr *)&c->coordinator,
        sizeof c->coordinator) == (ssize_t)sizeof p ? 0 : SIM_T2U_IO;
    volatile unsigned char *secret = p;
    for (size_t i = 0; i < sizeof p; ++i) secret[i] = 0;
    c->next_service = now_ms() + c->service_interval;
    return code;
}
static int heartbeat(struct sim_t2u_channel *c) {
    c->next_heartbeat = now_ms() + 3000;
    if (!c->ready) {
        /* Type 0 negotiates the peer's send window. Repeating it after the
         * handshake resets its congestion state; it is not a keepalive. */
        unsigned char p[20]; header(p, 0); memset(p + 16, 0, 4);
        struct timeval t; gettimeofday(&t, NULL);
        be32(p + 4, (uint32_t)t.tv_sec); be32(p + 8, (uint32_t)t.tv_usec);
        be16(p + 16, STREAM_CAP / 2048); be16(p + 18, STREAM_CAP / 2048);
        int code = emit(c, p, sizeof p);
        if (code) return code;
    }
    /* Signaling liveness is independent of VDT. The SDK's word at +4 is
     * an incrementing sequence, followed by version 121 and connected flag 1. */
    unsigned char alive[24] = {0};
    write_le32(alive, 0x07d22012); write_le32(alive + 4, ++c->peer_sequence);
    write_le32(alive + 8, 121); write_le32(alive + 12, 1);
    return emit(c, alive, sizeof alive);
}
static int ingest(struct sim_t2u_channel *c, const unsigned char *p, size_t n) {
    while (n) {
        if (!c->remaining) {
            size_t take = 8 - c->header_used;
            if (take > n) take = n;
            memcpy(c->record_header + c->header_used, p, take);
            c->header_used += take; p += take; n -= take;
            if (c->header_used < 8) break;
            c->record_id = read_le32(c->record_header);
            c->remaining = read_le32(c->record_header + 4);
            c->header_used = 0;
            if (c->remaining > 1024 * 1024) return SIM_T2U_PROTOCOL;
            if (c->record_id == 0xff02ffff) c->closed = 1;
            if (!c->remaining) continue;
        }
        size_t take = c->remaining < n ? c->remaining : n;
        if (c->record_id == 1) {
            memcpy(c->stream + c->used, p, take); c->used += take;
        }
        p += take; n -= take; c->remaining -= (uint32_t)take;
    }
    return 0;
}
static int acknowledge(struct sim_t2u_channel *c, uint32_t requested) {
    /* We retain only contiguous bytes. Request every discarded packet again,
     * including the tail named by a type-4 ACK request. Bound both work and UDP
     * size even when the peer supplies an arbitrarily large sequence number. */
    uint32_t end = c->rx_seen;
    if (requested > end) end = requested;
    uint32_t missing = end > c->rx_sequence ? end - c->rx_sequence : 0;
    if (missing > 256) missing = 256;
    unsigned char ack[16 + 256 * 4]; header(ack, 3);
    size_t window = (STREAM_CAP - c->used) / 2048;
    be16(ack + 4, (uint16_t)(window ? window : 1));
    be16(ack + 6, (uint16_t)missing);
    be32(ack + 8, c->rx_sequence);
    for (uint32_t i = 0; i < missing; ++i)
        be32(ack + 16 + i * 4, c->rx_sequence + i + 1);
    return emit(c, ack, 16 + missing * 4);
}
/* Process a bounded datagram without logging application data or credentials. */
static int step(struct sim_t2u_channel *c, uint64_t deadline) {
    uint64_t now = now_ms();
    if (now >= deadline) return SIM_T2U_TIMEOUT;
    if (c->service_interval && now >= c->next_service && service_heartbeat(c)) return SIM_T2U_IO;
    if (now >= c->next_heartbeat && heartbeat(c)) return SIM_T2U_IO;
    struct pollfd pollfd = {.fd = c->fd, .events = POLLIN};
    uint64_t left = deadline - now;
    int ready = poll(&pollfd, 1, left > 100 ? 100 : (int)left);
    if (ready < 0) return errno == EINTR ? 0 : SIM_T2U_IO;
    if (!ready) return 0;
    unsigned char p[2048];
    struct sockaddr_in from; socklen_t size = sizeof from;
    ssize_t n = recvfrom(c->fd, p, sizeof p, 0, (struct sockaddr *)&from, &size);
    if (n < 0) return errno == EAGAIN || errno == EINTR ? 0 : SIM_T2U_IO;
    if (c->service_interval && from.sin_addr.s_addr == c->coordinator.sin_addr.s_addr &&
        from.sin_port == c->coordinator.sin_port && n >= 52 && read_le32(p) == 0x03f52012) {
        if (!read_le32(p + 4)) return SIM_T2U_AUTH;
        tea_decrypt(p + 16, 32, p);
        int valid = memchr(p + 16, 0, 32) && !strcmp((char *)p + 16, c->service_key);
        volatile unsigned char *secret = p + 16;
        for (size_t i = 0; i < 32; ++i) secret[i] = 0;
        if (valid) {
            uint32_t seconds = read_le32(p + 8);
            if (seconds && seconds <= 120) c->service_interval = seconds * 500;
        }
        return 0;
    }
    if (from.sin_addr.s_addr != c->peer.sin_addr.s_addr || from.sin_port != c->peer.sin_port) return 0;
    if (n >= 120 && read_le32(p) == 0x07d02012) {
        write_le32(p, 0x07d12012); write_le32(p + 4, c->connection_id);
        memset(p + 8, 0, 100); memcpy(p + 8, c->uuid, strlen(c->uuid));
        write_le32(p + 112, 121);
        return emit(c, p, (size_t)n);
    }
    /* Transport FIN uses the short 12-byte header, not a data header. */
    if (n >= 12 && u16(p) == 0x1201 && u16(p + 2) == 0x1007) {
        c->closed = 1;
        return 0;
    }
    if (n < 16 || u16(p) != 0x1201 || (u16(p + 2) & 0xff00) != 0x1000) return 0;
    unsigned type = p[3];
    if (type == 0 && n >= 20) {
        p[3] = 1;
        return emit(c, p, 20);
    }
    if (type == 1) { c->ready = 1; return 0; }
    if (type == 5) { p[3] = 6; return emit(c, p, 16); }
    if (type == 3) { c->acknowledged = u32(p + 8); return 0; }
    if (type == 4) return acknowledge(c, u32(p + 4));
    if (type != 2) return 0;
    uint32_t sequence = u32(p + 4);
    size_t length = u16(p + 8);
    if (length > (size_t)n - 16) return SIM_T2U_PROTOCOL;
    /* Byte 11 counts retransmissions; a nonzero value is valid after loss. */
    if (p[10] > 1) return SIM_T2U_PROTOCOL;
    if (sequence > c->rx_seen) c->rx_seen = sequence;
    if (sequence == c->rx_sequence + 1) {
        /* Apply backpressure without acknowledging data we cannot retain. */
        if (length <= STREAM_CAP - c->used) {
            int code = ingest(c, p + 16, length);
            if (code) return code;
            c->rx_sequence = sequence;
        }
    }
    return acknowledge(c, 0);
}
static int transmit(struct sim_t2u_channel *c, const unsigned char *body, size_t n, uint64_t deadline) {
    if (n > CHUNK + 8) return SIM_T2U_INPUT;
    unsigned char packet[CHUNK + 24]; header(packet, 2);
    uint32_t sequence = ++c->tx_sequence;
    be32(packet + 4, sequence); be16(packet + 8, (uint16_t)n); packet[10] = 1; packet[12] = 1;
    memcpy(packet + 16, body, n);
    uint64_t resend = 0;
    while (now_ms() < deadline) {
        if (now_ms() >= resend) {
            int code = emit(c, packet, n + 16);
            if (code) return code;
            resend = now_ms() + 300;
        }
        if (c->acknowledged == sequence) return 0;
        int code = step(c, deadline);
        if (code) return code;
        if (c->closed) return SIM_T2U_IO;
    }
    return SIM_T2U_TIMEOUT;
}
struct sim_t2u_channel *vdt_open(int fd, const struct sockaddr_in *peer, const char *uuid,
    uint32_t connection_id, const char *remote_ip, uint16_t remote_port, uint32_t timeout_ms,
    struct sim_t2u_result *result, int *error) {
    struct sim_t2u_channel *c = calloc(1, sizeof *c);
    if (!c) return NULL;
    c->fd = fd; c->peer = *peer; c->connection_id = connection_id;
    memcpy(c->uuid, uuid, strlen(uuid));
    uint64_t deadline = now_ms() + timeout_ms;
    while (!c->ready) {
        int code = step(c, deadline);
        if (code) { if (error) *error = code; free(c); return NULL; }
    }
    if (result) result->transport_ready = 1;
    unsigned char open[32] = {0};
    write_le32(open, 0xff01ffff); write_le32(open + 4, 24); be16(open + 8, 1);
    memcpy(open + 10, remote_ip, strlen(remote_ip)); be16(open + 30, remote_port);
    int code = transmit(c, open, sizeof open, deadline);
    if (code) { if (error) *error = code; free(c); return NULL; }
    if (result) result->stream_opened = 1;
    return c;
}
int sim_t2u_write(struct sim_t2u_channel *c, const void *data, uint32_t size, uint32_t timeout_ms) {
    if (!c || !data || !size || !timeout_ms || timeout_ms > 120000 || size > WRITE_CAP) return SIM_T2U_INPUT;
    if (c->closed) return SIM_T2U_IO;
    uint64_t deadline = now_ms() + timeout_ms;
    const unsigned char *input = data;
    for (size_t offset = 0; offset < size;) {
        size_t n = size - offset; if (n > CHUNK) n = CHUNK;
        unsigned char body[CHUNK + 8];
        write_le32(body, 1); write_le32(body + 4, (uint32_t)n); memcpy(body + 8, input + offset, n);
        int code = transmit(c, body, n + 8, deadline);
        if (code) { c->closed = 1; return code; }
        offset += n;
    }
    return (int)size;
}
int sim_t2u_read(struct sim_t2u_channel *c, void *data, uint32_t cap, uint32_t timeout_ms) {
    if (!c || !data || !cap || !timeout_ms || timeout_ms > 120000) return SIM_T2U_INPUT;
    uint64_t deadline = now_ms() + timeout_ms;
    while (!c->used) {
        if (c->closed) return 0;
        int code = step(c, deadline);
        if (code) return code;
    }
    size_t n = c->used < cap ? c->used : cap;
    memcpy(data, c->stream, n); c->used -= n;
    memmove(c->stream, c->stream + n, c->used);
    return (int)n;
}
void sim_t2u_close(struct sim_t2u_channel *c) {
    if (!c) return;
    unsigned char body[32] = {0}, packet[48];
    write_le32(body, 0xff02ffff); write_le32(body + 4, 24); be16(body + 8, 1);
    header(packet, 2); be32(packet + 4, ++c->tx_sequence); be16(packet + 8, sizeof body);
    packet[10] = 1; packet[12] = 1; memcpy(packet + 16, body, sizeof body);
    (void)emit(c, packet, sizeof packet);
    /* The pinned SDK sends a transport FIN after the application close. Without
     * it, the remote peer can retain its session after our UDP socket disappears. */
    unsigned char finish[16]; header(finish, 7);
    struct timeval t; gettimeofday(&t, NULL);
    be32(finish + 4, (uint32_t)t.tv_sec); be32(finish + 8, (uint32_t)t.tv_usec);
    (void)emit(c, finish, 12);
    close(c->fd);
    volatile unsigned char *p = (volatile unsigned char *)c;
    for (size_t i = 0; i < sizeof *c; ++i) p[i] = 0;
    free(c);
}
