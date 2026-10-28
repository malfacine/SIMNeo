#ifndef SIM_T2U_VDT_H
#define SIM_T2U_VDT_H
#include "sim_t2u.h"
#include <netinet/in.h>
struct sim_t2u_channel *vdt_open(int fd, const struct sockaddr_in *peer,
    const char *uuid, uint32_t connection_id, const char *remote_ip,
    uint16_t remote_port, uint32_t timeout_ms, struct sim_t2u_result *result, int *error);
int vdt_configure_service(struct sim_t2u_channel *channel,
    const struct sockaddr_in *coordinator, const char *key, uint32_t interval_ms);
#endif
