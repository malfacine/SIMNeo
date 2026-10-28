#ifndef SIM_T2U_PROTOCOL_H
#define SIM_T2U_PROTOCOL_H
#include <stddef.h>
#include <stdint.h>
uint16_t read_le16(const unsigned char *p);
uint32_t read_le32(const unsigned char *p);
void write_le16(unsigned char *p, uint16_t v);
void write_le32(unsigned char *p, uint32_t v);
void tea_encrypt(unsigned char *p, size_t n, const unsigned char key[16]);
void tea_decrypt(unsigned char *p, size_t n, const unsigned char key[16]);
void md5_hex(const char *text, char out[33]);
size_t discovery_packet(unsigned char *out, size_t cap, uint32_t seconds);
size_t signal_packet(unsigned char *out, size_t cap, uint16_t opcode,
                     uint32_t seconds, uint32_t micros, const char *payload);
int signal_payload(const unsigned char *packet, size_t n, unsigned char *out, size_t cap);
#endif
