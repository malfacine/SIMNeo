#include "protocol.h"
#include <string.h>

/* RFC 1321, used solely for the legacy rendezvous token, never password storage. */
void md5_hex(const char *text, char out[33]) {
    static const uint32_t k[64] = {
        0xd76aa478,0xe8c7b756,0x242070db,0xc1bdceee,0xf57c0faf,0x4787c62a,0xa8304613,0xfd469501,
        0x698098d8,0x8b44f7af,0xffff5bb1,0x895cd7be,0x6b901122,0xfd987193,0xa679438e,0x49b40821,
        0xf61e2562,0xc040b340,0x265e5a51,0xe9b6c7aa,0xd62f105d,0x02441453,0xd8a1e681,0xe7d3fbc8,
        0x21e1cde6,0xc33707d6,0xf4d50d87,0x455a14ed,0xa9e3e905,0xfcefa3f8,0x676f02d9,0x8d2a4c8a,
        0xfffa3942,0x8771f681,0x6d9d6122,0xfde5380c,0xa4beea44,0x4bdecfa9,0xf6bb4b60,0xbebfbc70,
        0x289b7ec6,0xeaa127fa,0xd4ef3085,0x04881d05,0xd9d4d039,0xe6db99e5,0x1fa27cf8,0xc4ac5665,
        0xf4292244,0x432aff97,0xab9423a7,0xfc93a039,0x655b59c3,0x8f0ccc92,0xffeff47d,0x85845dd1,
        0x6fa87e4f,0xfe2ce6e0,0xa3014314,0x4e0811a1,0xf7537e82,0xbd3af235,0x2ad7d2bb,0xeb86d391};
    static const unsigned s[64] = {
        7,12,17,22,7,12,17,22,7,12,17,22,7,12,17,22,
        5,9,14,20,5,9,14,20,5,9,14,20,5,9,14,20,
        4,11,16,23,4,11,16,23,4,11,16,23,4,11,16,23,
        6,10,15,21,6,10,15,21,6,10,15,21,6,10,15,21};
    uint32_t h[4] = {0x67452301,0xefcdab89,0x98badcfe,0x10325476};
    size_t len = strlen(text), total = ((len + 8) / 64 + 1) * 64;
    for (size_t offset = 0; offset < total; offset += 64) {
        unsigned char block[64] = {0};
        for (size_t j = 0; j < 64; ++j) {
            size_t at = offset + j;
            if (at < len) block[j] = (unsigned char)text[at];
            else if (at == len) block[j] = 0x80;
            if (at >= total - 8) block[j] = (unsigned char)((uint64_t)len * 8 >> (8 * (at - (total - 8))));
        }
        uint32_t a = h[0], b = h[1], c = h[2], d = h[3];
        for (unsigned i = 0; i < 64; ++i) {
            uint32_t f; unsigned g;
            if (i < 16) { f = (b & c) | (~b & d); g = i; }
            else if (i < 32) { f = (d & b) | (~d & c); g = (5 * i + 1) % 16; }
            else if (i < 48) { f = b ^ c ^ d; g = (3 * i + 5) % 16; }
            else { f = c ^ (b | ~d); g = 7 * i % 16; }
            uint32_t v = a + f + k[i] + read_le32(block + g * 4);
            a = d; d = c; c = b;
            b += (v << s[i]) | (v >> (32 - s[i]));
        }
        h[0] += a; h[1] += b; h[2] += c; h[3] += d;
    }
    static const char hex[] = "0123456789abcdef";
    for (unsigned i = 0; i < 16; ++i) {
        unsigned v = (h[i / 4] >> (8 * (i % 4))) & 255;
        out[i * 2] = hex[v >> 4]; out[i * 2 + 1] = hex[v & 15];
    }
    out[32] = 0;
}

uint16_t read_le16(const unsigned char *p) {
    return (uint16_t)((uint16_t)p[0] | (uint16_t)p[1] << 8);
}
uint32_t read_le32(const unsigned char *p) {
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}
void write_le16(unsigned char *p, uint16_t v) {
    p[0] = (unsigned char)v; p[1] = (unsigned char)(v >> 8);
}
void write_le32(unsigned char *p, uint32_t v) {
    for (unsigned i = 0; i < 4; ++i) p[i] = (unsigned char)(v >> (8 * i));
}

/* The observed wire format uses the original TEA, not XTEA or XXTEA. */
void tea_encrypt(unsigned char *p, size_t n, const unsigned char key[16]) {
    uint32_t k[4];
    for (unsigned i = 0; i < 4; ++i) k[i] = read_le32(key + i * 4);
    for (size_t i = 0; i + 8 <= n; i += 8) {
        uint32_t a = read_le32(p + i), b = read_le32(p + i + 4), sum = 0;
        for (unsigned round = 0; round < 32; ++round) {
            sum += UINT32_C(0x9e3779b9);
            a += ((b << 4) + k[0]) ^ (b + sum) ^ ((b >> 5) + k[1]);
            b += ((a << 4) + k[2]) ^ (a + sum) ^ ((a >> 5) + k[3]);
        }
        write_le32(p + i, a); write_le32(p + i + 4, b);
    }
}
void tea_decrypt(unsigned char *p, size_t n, const unsigned char key[16]) {
    uint32_t k[4];
    for (unsigned i = 0; i < 4; ++i) k[i] = read_le32(key + i * 4);
    for (size_t i = 0; i + 8 <= n; i += 8) {
        uint32_t a = read_le32(p + i), b = read_le32(p + i + 4), sum = UINT32_C(0xc6ef3720);
        for (unsigned round = 0; round < 32; ++round) {
            b -= ((a << 4) + k[2]) ^ (a + sum) ^ ((a >> 5) + k[3]);
            a -= ((b << 4) + k[0]) ^ (b + sum) ^ ((b >> 5) + k[1]);
            sum -= UINT32_C(0x9e3779b9);
        }
        write_le32(p + i, a); write_le32(p + i + 4, b);
    }
}
size_t discovery_packet(unsigned char *out, size_t cap, uint32_t seconds) {
    if (cap < 16) return 0;
    write_le32(out, UINT32_C(0x13892012));
    write_le32(out + 4, seconds);
    write_le32(out + 8, UINT32_C(0x00320001));
    write_le32(out + 12, 1);
    return 16;
}
size_t signal_packet(unsigned char *out, size_t cap, uint16_t opcode,
                     uint32_t seconds, uint32_t micros, const char *payload) {
    size_t raw = strlen(payload) + 1;
    if (raw > 65528) return 0;
    size_t enc = (raw + 7) & ~(size_t)7;
    if (cap < 20 + enc) return 0;
    memset(out, 0, 20 + enc);
    write_le32(out, (uint32_t)opcode << 16 | 0x2012);
    write_le32(out + 4, seconds); write_le32(out + 8, micros);
    write_le16(out + 16, (uint16_t)raw); write_le16(out + 18, (uint16_t)enc);
    memcpy(out + 20, payload, raw);
    tea_encrypt(out + 20, enc, out);
    return 20 + enc;
}
int signal_payload(const unsigned char *packet, size_t n, unsigned char *out, size_t cap) {
    if (n < 20 || read_le16(packet) != 0x2012) return -1;
    size_t raw = read_le16(packet + 16), enc = read_le16(packet + 18);
    if (!raw || !enc || enc % 8 || raw > enc || enc > n - 20 || enc > cap) return -1;
    memcpy(out, packet + 20, enc);
    tea_decrypt(out, enc, packet);
    if (out[raw - 1] != 0) return -1;
    return (int)raw;
}
