#include "sim_intelbras.h"
#include <dhnetsdk.h>
#include <cstring>
#include <mutex>

extern "C" int sim_intelbras_probe(const char *host, uint16_t port, const char *username,
                                    const char *password, int mode,
                                    sim_intelbras_result *result) {
    if (!result) return -1;
    std::memset(result, 0, sizeof *result);
    if (host && (!*host || !port || !username || !password ||
                 (mode != EM_LOGIN_SPEC_CAP_TCP &&
                  mode != EM_LOGIN_SPEC_CAP_P2P))) return -1;
    // NetSDK has process-global state. Serialize these short validation calls.
    static std::mutex mutex;
    const std::lock_guard<std::mutex> lock(mutex);
    result->sdk_version = CLIENT_GetSDKVersion();
    if (!CLIENT_Init(nullptr, 0)) {
        result->last_error = CLIENT_GetLastError();
        return -2;
    }
    result->initialized = 1;
    int code = 0;
    if (host) {
        CLIENT_SetConnectTime(5000, 1);
        NET_DEVICEINFO_Ex info = {};
        LLONG handle = CLIENT_LoginEx2(host, port, username, password,
                                       static_cast<EM_LOGIN_SPAC_CAP_TYPE>(mode),
                                       nullptr, &info, &result->login_error);
        if (!handle) {
            result->last_error = CLIENT_GetLastError();
            code = -3;
        } else {
            result->logged_in = 1;
            result->channels = info.nChanNum;
            if (!CLIENT_Logout(handle)) {
                result->last_error = CLIENT_GetLastError();
                code = -4;
            }
        }
    }
    CLIENT_Cleanup();
    return code;
}
