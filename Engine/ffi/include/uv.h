// uv.h: C ABI of the Unveil engine bridge. Version 1.
#pragma once
#include <stdbool.h>
#include <stdint.h>

#define UV_ABI_VERSION 1

typedef enum UVStatus {
    UV_OK                    =  0,
    UV_ERR_INVALID_ARGUMENT  = -1,
    UV_ERR_UNKNOWN_COMMAND   = -2,
    UV_ERR_ENGINE            = -3,
    UV_ERR_PANIC             = -4,
    UV_ERR_SUSPENDED         = -5,
    UV_ERR_IO                = -6,
    UV_ERR_TIMEOUT           = -7,
} UVStatus;

typedef struct UVSession UVSession;

/// Called on the engine thread; `rgba` is valid only during the call.
typedef void (*uv_frame_cb)(void *ctx, const uint8_t *rgba, uint32_t width, uint32_t height,
                            uint32_t stride, uint64_t generation, bool draft);

uint32_t    uv_abi_version(void);
UVSession  *uv_session_new(const char *data_dir, uint64_t memory_budget);
void        uv_session_free(UVSession *session);

int32_t     uv_execute(UVSession *session, const char *command, const char *params_json,
                       char **result_json);
void        uv_string_free(char *string);
const char *uv_last_error(void);

uint64_t    uv_request_preview(UVSession *session, uint32_t max_pixels, bool draft,
                               uv_frame_cb callback, void *ctx);

int32_t     uv_suspend(UVSession *session);
int32_t     uv_resume(UVSession *session);
void        uv_set_memory_budget(UVSession *session, uint64_t bytes);
