#ifndef SIM_INTELBRAS_LINUX_H
#define SIM_INTELBRAS_LINUX_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
struct sim_intelbras_result {
    uint32_t sdk_version;
    uint32_t initialized;
    uint32_t logged_in;
    uint32_t last_error;
    int32_t login_error;
    int32_t channels;
};
/* Isolated SDK validation. host == NULL tests init/cleanup only.
 * login_mode: use the enum in the supplied SDK header. Caller owns strings.
 * No persistent configuration, logs, device changes or recording commands. */
int sim_intelbras_probe(const char *host, uint16_t port, const char *username,
                         const char *password, int login_mode,
                         struct sim_intelbras_result *result);
#ifdef __cplusplus
}
#endif
#endif
