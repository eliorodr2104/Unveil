//! UVStatus mirrors the status codes of uv.h; Failure carries one with its message.

use lightcraft_engine::EngineError;

/// UVStatus is what every int32_t-returning export answers. Values are fixed by uv.h.
// The names mirror uv.h, so they keep its C spelling.
#[allow(non_camel_case_types)]
#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UVStatus {
    UV_OK = 0,
    UV_ERR_INVALID_ARGUMENT = -1,
    UV_ERR_UNKNOWN_COMMAND = -2,
    UV_ERR_ENGINE = -3,
    UV_ERR_PANIC = -4,
    UV_ERR_SUSPENDED = -5,
    UV_ERR_IO = -6,
    UV_ERR_TIMEOUT = -7,
}

/// Failure is a failed export: the status to return and the text uv_last_error will show.
#[derive(Debug)]
pub struct Failure {
    pub status: UVStatus,
    pub message: String,
}

impl Failure {
    pub fn new(status: UVStatus, message: impl Into<String>) -> Failure {
        Failure {
            status,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Failure {
        Failure::new(UVStatus::UV_ERR_INVALID_ARGUMENT, message)
    }

    /// engine maps an engine error to its status: only an unknown command has its own code.
    pub fn engine(e: EngineError) -> Failure {
        let status = match e {
            EngineError::UnknownCommand(_) => UVStatus::UV_ERR_UNKNOWN_COMMAND,
            _ => UVStatus::UV_ERR_ENGINE,
        };
        Failure::new(status, e.to_string())
    }
}
