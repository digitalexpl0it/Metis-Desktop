#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** Shared-memory frame header written by the Rust capture thread. */
typedef struct MetisFrameHeader
{
	uint32_t magic;   /* 'MFRM' little-endian */
	uint32_t width;
	uint32_t height;
	uint32_t stride;  /* bytes per row */
	uint32_t seq;
	uint32_t ready;   /* 1 when pixel data is valid */
	uint32_t reserved[2];
} MetisFrameHeader;

#define METIS_FRAME_MAGIC 0x4D46524Du /* 'MFRM' */

/**
 * Run the FreeRDP shadow server (blocking) using portal frames from `shm_path`.
 *
 * @param shm_path   path to the mmap'd MetisFrameHeader + BGRX pixels
 * @param input_path Unix socket path for JSON input events (created by Rust)
 * @param port       TCP listen port (usually 3389)
 * @param password   cleartext password for NLA/RDP auth (may be empty → no auth)
 * @return 0 on clean stop, non-zero on failure
 */
int metis_rdp_server_run(const char* shm_path, const char* input_path, uint16_t port,
                         const char* password);

/** Request the server thread to stop (safe from another thread). */
void metis_rdp_server_stop(void);

#ifdef __cplusplus
}
#endif
