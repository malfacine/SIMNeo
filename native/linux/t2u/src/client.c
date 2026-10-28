#define _POSIX_C_SOURCE 200809L
#include "sim_t2u.h"
#include "protocol.h"
#include "vdt.h"
#include <arpa/inet.h>
#include <errno.h>
#include <netdb.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#define MAX_SERVERS 32
static uint64_t monotonic_ms(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000 + (uint64_t)t.tv_nsec / 1000000;
}
static void wipe(void *p, size_t n) {
    volatile unsigned char *v = p;
    while (n--) *v++ = 0;
}
static int same_endpoint(const struct sockaddr_in *a, const struct sockaddr_in *b) {
    return a->sin_addr.s_addr == b->sin_addr.s_addr && a->sin_port == b->sin_port;
}
static void add_endpoint(struct sockaddr_in *servers, size_t *count, const struct sockaddr_in *a) {
    for (size_t i = 0; i < *count; ++i) if (same_endpoint(servers + i, a)) return;
    if (*count < MAX_SERVERS) servers[(*count)++] = *a;
}
static int send_packet(int fd, const struct sockaddr_in *to, const unsigned char *p, size_t n) {
    return sendto(fd, p, n, 0, (const struct sockaddr *)to, sizeof *to) == (ssize_t)n ? 0 : -1;
}
static void read_servers(unsigned char *p, size_t n, struct sockaddr_in *servers, size_t *count) {
    /* DLL: 5002 uses a u32 encrypted byte count at +16 and text at +20.
     * Each line: IPv4:port|IPv6|hostname. Only IPv4 is implemented in this spike. */
    if (n < 21) return;
    size_t enc = read_le32(p + 16);
    if (enc) {
        if (enc % 8 || enc > n - 20) return;
        tea_decrypt(p + 20, enc, p);
    }
    if (!memchr(p + 20, 0, n - 20)) return;
    char *save = NULL;
    for (char *line = strtok_r((char *)p + 20, "\n", &save); line;
         line = strtok_r(NULL, "\n", &save)) {
        char *end = strchr(line, '|');
        if (end) *end = 0;
        char *colon = strchr(line, ':');
        if (!colon) continue;
        *colon++ = 0;
        char *tail;
        long port = strtol(colon, &tail, 10);
        if (*tail || port < 1 || port > 65535) continue;
        struct sockaddr_in a = {.sin_family = AF_INET, .sin_port = htons((uint16_t)port)};
        if (inet_pton(AF_INET, line, &a.sin_addr) == 1) add_endpoint(servers, count, &a);
    }
}
static int probe(const char *server, uint16_t port, const char *key,
                 const char *serial, const char *tunnel_password,
                 uint32_t timeout_ms, struct sim_t2u_result *result,
                 const char *remote_ip, uint16_t remote_port, struct sim_t2u_channel **channel) {
    if (!result) return SIM_T2U_INPUT;
    memset(result, 0, sizeof *result);
    if (!server || !*server || !port || !key || !*key || strlen(key) >= 32 ||
        timeout_ms < 100 || timeout_ms > 120000) return SIM_T2U_INPUT;
    if (serial && (!*serial || strlen(serial) >= 64 || !tunnel_password ||
                   strlen(tunnel_password) >= 36)) return SIM_T2U_INPUT;
    char service[6];
    snprintf(service, sizeof service, "%u", port);
    struct addrinfo hints = {.ai_family = AF_INET, .ai_socktype = SOCK_DGRAM}, *resolved = NULL;
    if (getaddrinfo(server, service, &hints, &resolved)) return SIM_T2U_DNS;
    struct sockaddr_in servers[MAX_SERVERS];
    size_t count = 0;
    for (struct addrinfo *a = resolved; a; a = a->ai_next)
        if (a->ai_addrlen == sizeof(struct sockaddr_in))
            add_endpoint(servers, &count, (const struct sockaddr_in *)a->ai_addr);
    freeaddrinfo(resolved);
    if (!count) return SIM_T2U_DNS;
    int fd = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
    if (fd < 0) return SIM_T2U_IO;
    struct sockaddr_in local = {.sin_family = AF_INET, .sin_addr.s_addr = htonl(INADDR_ANY)};
    if (bind(fd, (struct sockaddr *)&local, sizeof local)) { close(fd); return SIM_T2U_IO; }
    unsigned char p[4096];
    char uuid[100] = {0};
    /* The DLL generates its 32-character client ID after the first STUN reply;
     * it does not wait for 1007. An empty ID is ignored by the real service. */
    unsigned char random_id[16];
    if (getrandom(random_id, sizeof random_id, 0) != sizeof random_id) {
        close(fd); return SIM_T2U_IO;
    }
    for (size_t i = 0; i < sizeof random_id; ++i)
        snprintf(uuid + i * 2, 3, "%02x", random_id[i]);
    wipe(random_id, sizeof random_id);
    uint32_t connection_id;
    if (getrandom(&connection_id, sizeof connection_id, 0) != sizeof connection_id) {
        close(fd); return SIM_T2U_IO;
    }
    connection_id = (connection_id & UINT32_C(0x7ffffffe)) + 1;
    struct sockaddr_in coordinator = servers[0], peers[MAX_SERVERS];
    size_t peer_count = 0;
    char mapped_ip[16] = {0};
    socklen_t local_size = sizeof local;
    if (getsockname(fd, (struct sockaddr *)&local, &local_size)) { close(fd); return SIM_T2U_IO; }
    uint64_t next_peer = 0, next_request = 0;
    uint32_t service_interval = 15000;
    struct sockaddr_in relay = {0};
    uint32_t remote_connection_id = 0;
    int relay_registered = 0;
    uint64_t next_relay = 0;
    int code = SIM_T2U_TIMEOUT;
    uint64_t deadline = monotonic_ms() + timeout_ms, next_send = 0;
    while (monotonic_ms() < deadline) {
        uint64_t now = monotonic_ms();
        if (serial && relay.sin_port && !relay_registered && now >= next_relay) {
            memset(p, 0, 212); write_le32(p, 0x0bb92012);
            memcpy(p + 4, uuid, strlen(uuid)); write_le32(p + 104, connection_id);
            memcpy(p + 108, serial, strlen(serial)); write_le32(p + 208, remote_connection_id);
            if (send_packet(fd, &relay, p, 212)) { code = SIM_T2U_IO; break; }
            next_relay = now + 1000;
        }
        if (serial && result->authenticated && !result->peer_notices && now >= next_request) {
            memset(p, 0, 588);
            write_le32(p, 0x03ec2012); write_le32(p + 4, connection_id);
            memcpy(p + 8, uuid, strlen(uuid)); memcpy(p + 108, serial, strlen(serial));
            memcpy(p + 208, mapped_ip, sizeof mapped_ip);
            write_le16(p + 224, 0);
            write_le16(p + 226, ntohs(local.sin_port));
            write_le32(p + 352, (uint32_t)time(NULL));
            write_le32(p + 356, 121); write_le32(p + 360, 5);
            char token[33];
            md5_hex(tunnel_password, token); memcpy(p + 468, token, 33);
            wipe(token, sizeof token);
            if (send_packet(fd, &coordinator, p, 588)) { code = SIM_T2U_IO; break; }
            next_request = now + 2000;
        }
        if (serial && now >= next_peer) {
            for (size_t i = 0; i < peer_count; ++i) {
                int via_relay = relay.sin_port && same_endpoint(peers + i, &relay);
                if (via_relay && !relay_registered) continue;
                memset(p, 0, 184);
                write_le32(p, 0x07d02012); write_le32(p + 4, connection_id);
                memcpy(p + 8, uuid, strlen(uuid));
                write_le32(p + 108, 1); write_le32(p + 112, 121);
                write_le32(p + 116, 1); write_le32(p + 120, via_relay ? 3 : 1);
                char peer_ip[INET_ADDRSTRLEN];
                if (inet_ntop(AF_INET, &peers[i].sin_addr, peer_ip, sizeof peer_ip))
                    snprintf((char *)p + 124, 60, "%s:%u", peer_ip, ntohs(peers[i].sin_port));
                if (send_packet(fd, peers + i, p, 184)) { code = SIM_T2U_IO; goto done; }
            }
            next_peer = now + 500;
        }
        if (now >= next_send) {
            struct timeval tv;
            gettimeofday(&tv, NULL);
            size_t destinations = result->authenticated ? 1 : count;
            for (size_t i = 0; i < destinations; ++i) {
                const struct sockaddr_in *destination = result->authenticated ? &coordinator : servers + i;
                size_t n = discovery_packet(p, sizeof p, (uint32_t)tv.tv_sec);
                if (!result->authenticated && send_packet(fd, destination, p, n)) { code = SIM_T2U_IO; goto done; }
                memset(p, 0, 28);
                write_le32(p, 0x03e82012); write_le32(p + 16, (uint32_t)tv.tv_sec);
                write_le32(p + 20, (uint32_t)tv.tv_usec);
                if (!result->authenticated && send_packet(fd, destination, p, 28)) { code = SIM_T2U_IO; goto done; }
                memset(p, 0, 160);
                write_le32(p, 0x03f42012); write_le32(p + 4, (uint32_t)tv.tv_sec);
                if (getrandom(p + 8, 8, 0) != 8) { code = SIM_T2U_IO; goto done; }
                memcpy(p + 16, key, strlen(key));
                tea_encrypt(p + 16, 32, p);
                memcpy(p + 48, uuid, sizeof uuid);
                write_le32(p + 148, 121);
                /* Legacy wire compatibility experiment: the reference client
                 * measured on this host sends symmetric NAT (5), platform 3.
                 * This does not make the native library a Windows binary. */
                write_le32(p + 152, 5);
                write_le32(p + 156, 3);
                if (send_packet(fd, destination, p, 160)) { code = SIM_T2U_IO; goto done; }
                wipe(p, sizeof p);
                /* Pinned DLL's nonce provider at RVA 0x21ab0 is a no-op;
                 * its 1023 payload is uuid|key| with an empty third field. */
                char auth[160];
                snprintf(auth, sizeof auth, "%s|%s|", uuid, key);
                n = signal_packet(p, sizeof p, 1023, (uint32_t)tv.tv_sec,
                                  (uint32_t)tv.tv_usec, auth);
                wipe(auth, sizeof auth);
                if (!result->authenticated && (!n || send_packet(fd, destination, p, n))) { code = SIM_T2U_IO; goto done; }
                wipe(p, sizeof p);
            }
            next_send = now + 2000;
        }
        struct pollfd pollfd = {.fd = fd, .events = POLLIN};
        uint64_t before_poll = monotonic_ms();
        if (before_poll >= deadline) break;
        int wait = (int)(deadline - before_poll);
        if (wait > 100) wait = 100;
        int ready = poll(&pollfd, 1, wait);
        if (ready < 0 && errno != EINTR) { code = SIM_T2U_IO; break; }
        if (ready <= 0) continue;
        struct sockaddr_in from;
        socklen_t fromlen = sizeof from;
        ssize_t received = recvfrom(fd, p, sizeof p, 0, (struct sockaddr *)&from, &fromlen);
        if (received < 0 && (errno == EAGAIN || errno == EINTR)) continue;
        if (received < 0) { code = SIM_T2U_IO; break; }
        int trusted = 0, trusted_peer = 0;
        for (size_t i = 0; i < count; ++i) if (same_endpoint(servers + i, &from)) trusted = 1;
        for (size_t i = 0; i < peer_count; ++i) if (same_endpoint(peers + i, &from)) trusted_peer = 1;
        /* Symmetric NAT may reply from a new port on a coordinator-supplied IP.
         * Match both that IP and the requested serial before admitting it. */
        if (!trusted_peer && serial && result->authenticated && received >= 120 &&
            read_le16(p) == 0x2012 && (read_le16(p + 2) == 2000 || read_le16(p + 2) == 2001) &&
            memchr(p + 8, 0, 100) && !strcmp((char *)p + 8, serial)) {
            for (size_t i = 0; i < peer_count; ++i)
                if (peers[i].sin_addr.s_addr == from.sin_addr.s_addr) trusted_peer = 1;
            if (trusted_peer) add_endpoint(peers, &peer_count, &from);
        }
        if ((!trusted && !trusted_peer) || received < 4 || read_le16(p) != 0x2012) continue;
        ++result->received;
        result->last_opcode = read_le16(p + 2);
        size_t n = (size_t)received;
        if (trusted_peer) {
            uint32_t op = result->last_opcode;
            if (op == 3002 && n >= 8 && same_endpoint(&from, &relay)) {
                relay_registered = 1;
                result->relay_registered = 1;
                next_peer = 0;
                continue;
            }
            if ((op == 2000 || op == 2001) && n >= 120 && serial &&
                memchr(p + 8, 0, 100) && !strcmp((char *)p + 8, serial)) {
                ++result->peer_hellos;
                result->peer_connected = 1;
                result->via_relay = relay.sin_port && same_endpoint(&from, &relay);
                if (op == 2000) {
                    write_le32(p, 0x07d12012); write_le32(p + 4, connection_id);
                    memset(p + 8, 0, 100); memcpy(p + 8, uuid, strlen(uuid));
                    write_le32(p + 112, 121);
                    if (send_packet(fd, &from, p, 120)) { code = SIM_T2U_IO; break; }
                }
                if (channel) {
                    uint64_t now = monotonic_ms();
                    if (now >= deadline) break;
                    *channel = vdt_open(fd, &from, uuid, connection_id, remote_ip,
                                        remote_port, (uint32_t)(deadline - now), result, &code);
                    if (*channel) {
                        fd = -1;
                        code = vdt_configure_service(*channel, &coordinator, key, service_interval);
                        if (code) { sim_t2u_close(*channel); *channel = NULL; }
                    }
                    goto done;
                }
                memset(p, 0, 72); write_le32(p, 0x07d72012);
                memcpy(p + 36, tunnel_password, strlen(tunnel_password));
                if (send_packet(fd, &from, p, 72)) { code = SIM_T2U_IO; break; }
            } else if (op == 2008 && n >= 24) {
                if (read_le32(p + 4) != 1) { code = SIM_T2U_AUTH; break; }
                result->peer_authenticated = 1; code = SIM_T2U_OK; break;
            }
            wipe(p, sizeof p);
            continue;
        }
        switch (result->last_opcode) {
        case 5002:
            read_servers(p, n, servers, &count);
            result->discovered = (uint32_t)count;
            break;
        case 1001:
            if (n >= 32 && memchr(p + 4, 0, 16)) {
                result->mapped = 1;
                memcpy(mapped_ip, p + 4, sizeof mapped_ip);
            }
            break;
        case 1004:
        case 1015:
            /* Only a rendezvous from an authenticated coordinator may supply
             * peer destinations. UUIDs must refer to this requested device. */
            if (serial && result->authenticated &&
                n >= 588 && memchr(p + 8, 0, 100) && memchr(p + 108, 0, 100) &&
                !strcmp((char *)p + 8, serial) && !strcmp((char *)p + 108, uuid) &&
                memchr(p + 208, 0, 16)) {
                struct sockaddr_in a = {.sin_family = AF_INET,
                    .sin_port = htons(read_le16(p + 224))};
                if (!a.sin_port) a.sin_port = htons(read_le16(p + 226));
                if (a.sin_port && inet_pton(AF_INET, (char *)p + 208, &a.sin_addr) == 1) {
                    add_endpoint(peers, &peer_count, &a);
                    if (ntohs(a.sin_port) > 1) {
                        a.sin_port = htons((uint16_t)(ntohs(a.sin_port) - 1));
                        add_endpoint(peers, &peer_count, &a);
                    }
                }
                if (memchr(p + 228, 0, 100) && read_le16(p + 226)) {
                    char *save = NULL;
                    for (char *ip = strtok_r((char *)p + 228, ",", &save); ip;
                         ip = strtok_r(NULL, ",", &save)) {
                        a.sin_port = htons(read_le16(p + 226));
                        if (inet_pton(AF_INET, ip, &a.sin_addr) == 1)
                            add_endpoint(peers, &peer_count, &a);
                    }
                }
                ++result->peer_notices;
                remote_connection_id = read_le32(p + 4);
                if (memchr(p + 332, 0, 16) && read_le16(p + 348)) {
                    struct sockaddr_in proposed = {.sin_family = AF_INET,
                        .sin_port = htons(read_le16(p + 348))};
                    if (inet_pton(AF_INET, (char *)p + 332, &proposed.sin_addr) == 1) {
                        if (!same_endpoint(&proposed, &relay)) relay_registered = 0;
                        relay = proposed;
                        add_endpoint(peers, &peer_count, &relay);
                    }
                }
            }
            break;
        case 1007:
            ++result->uuid_responses;
            if (n >= 104 && memchr(p + 4, 0, 100)) memcpy(uuid, p + 4, 100);
            break;
        case 1022:
            ++result->v2_responses;
            if (n >= 16) result->v2_error = read_le32(p + 12);
            break;
        case 1013:
            ++result->key_responses;
            if (n < 52) break;
            if (read_le32(p + 4) == 0) { code = SIM_T2U_AUTH; goto done; }
            tea_decrypt(p + 16, 32, p);
            if (!memchr(p + 16, 0, 32) || strcmp((char *)p + 16, key)) break;
            uint32_t seconds = read_le32(p + 8);
            if (seconds && seconds <= 120) service_interval = seconds * 500;
            if (!result->authenticated) coordinator = from;
            result->authenticated = 1;
            if (!serial) { code = SIM_T2U_OK; goto done; }
            break;
        default: break;
        }
        wipe(p, sizeof p);
    }
done:
    wipe(p, sizeof p); wipe(uuid, sizeof uuid);
    if (fd >= 0) close(fd);
    return code;
}
uint32_t sim_t2u_abi_version(void) { return 1; }

int sim_t2u_probe(const char *server, uint16_t port, const char *key,
                  uint32_t timeout_ms, struct sim_t2u_result *result) {
    return probe(server, port, key, NULL, NULL, timeout_ms, result, NULL, 0, NULL);
}
int sim_t2u_peer_probe(const char *server, uint16_t port, const char *key,
                       const char *serial, const char *tunnel_password,
                       uint32_t timeout_ms, struct sim_t2u_result *result) {
    if (!serial) return SIM_T2U_INPUT;
    return probe(server, port, key, serial, tunnel_password, timeout_ms, result, NULL, 0, NULL);
}
int sim_t2u_open(const char *server, uint16_t port, const char *key,
                 const char *serial, const char *tunnel_password,
                 const char *remote_ip, uint16_t remote_port, uint32_t timeout_ms,
                 struct sim_t2u_channel **channel, struct sim_t2u_result *result) {
    if (!channel) return SIM_T2U_INPUT;
    *channel = NULL;
    if (!serial || !remote_ip || !*remote_ip || strlen(remote_ip) > 19 || !remote_port) return SIM_T2U_INPUT;
    return probe(server, port, key, serial, tunnel_password, timeout_ms, result, remote_ip, remote_port, channel);
}
