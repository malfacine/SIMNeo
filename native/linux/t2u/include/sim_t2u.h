#ifndef SIM_T2U_LINUX_H
#define SIM_T2U_LINUX_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

/* Native Linux signaling and byte-stream API; not the proprietary DLL ABI. */
uint32_t sim_t2u_abi_version(void);
enum sim_t2u_error {
    SIM_T2U_OK = 0,
    SIM_T2U_INPUT = -1,
    SIM_T2U_DNS = -2,
    SIM_T2U_IO = -3,
    SIM_T2U_TIMEOUT = -4,
    SIM_T2U_AUTH = -5,
    SIM_T2U_PROTOCOL = -6
};
struct sim_t2u_result {
    uint32_t received;
    uint32_t discovered;
    uint32_t mapped;
    uint32_t authenticated;
    uint32_t last_opcode;
    uint32_t key_responses;
    uint32_t v2_responses;
    uint32_t uuid_responses;
    uint32_t v2_error;
    uint32_t peer_notices;
    uint32_t peer_hellos;
    uint32_t peer_authenticated;
    uint32_t relay_registered;
    uint32_t peer_connected;
    uint32_t transport_ready;
    uint32_t stream_opened;
    uint32_t via_relay;
};

/* Synchronous, bounded network probe. key is borrowed, never logged or stored.
 * Success means a validated key response, NOT a working device tunnel.
 * Call from a worker; it may block up to timeout_ms after DNS resolution. */
int sim_t2u_probe(const char *server, uint16_t port, const char *key,
                  uint32_t timeout_ms, struct sim_t2u_result *result);
/* Legacy 2007/2008 peer-login diagnostic. Modern devices should use open(),
 * which validates the token-based path and opens the actual VDT stream. */
int sim_t2u_peer_probe(const char *server, uint16_t port, const char *key,
                       const char *serial, const char *tunnel_password,
                       uint32_t timeout_ms, struct sim_t2u_result *result);
struct sim_t2u_channel;
/* One independent byte-stream channel. Calls on a handle must be serialized.
 * read/write drive keepalive; idle handles must be polled at least every 3 s.
 * No listener or Windows dependency. Release with close even after an error. */
int sim_t2u_open(const char *server, uint16_t port, const char *key,
                 const char *serial, const char *tunnel_password,
                 const char *remote_ip, uint16_t remote_port, uint32_t timeout_ms,
                 struct sim_t2u_channel **channel, struct sim_t2u_result *result);
int sim_t2u_write(struct sim_t2u_channel *channel, const void *data, uint32_t size, uint32_t timeout_ms);
int sim_t2u_read(struct sim_t2u_channel *channel, void *data, uint32_t capacity, uint32_t timeout_ms);
void sim_t2u_close(struct sim_t2u_channel *channel);
#ifdef __cplusplus
}
#endif
#endif
