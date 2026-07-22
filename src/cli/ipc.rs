//! IPC 协议：CLI client ↔ daemon 之间的消息格式。
//!
//! 用 serde_json over unix socket 通信（每条消息一行 JSON，以 `\n` 分隔）。
//! CliCommand 和 OutputFormat 直接 derive serde，用 tagged enum 序列化。

use crate::cli::{CliCommand, OutputFormat};
use serde::{Deserialize, Serialize};

/// Client → Daemon 请求。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// 已解析的 CliCommand。
    pub command: CliCommand,
    /// 输出格式。
    pub format: OutputFormat,
}

/// Daemon → Client 响应。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    /// 格式化输出（ok=true 时）。
    pub output: String,
    /// 错误信息（ok=false 时）。
    pub error: String,
}

impl Response {
    pub fn ok(output: String) -> Self {
        Self {
            ok: true,
            output,
            error: String::new(),
        }
    }

    pub fn err(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: String::new(),
            error: error.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::PaneId;

    #[test]
    fn request_serializes_command() {
        let req = Request {
            command: CliCommand::SplitPane {
                horizontal: true,
                target: Some(PaneId(1)),
                size: None,
            },
            format: OutputFormat::Json,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("SplitPane"));
        assert!(json.contains("true"));
        // round-trip
        let req2: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(req2.format, OutputFormat::Json));
    }

    #[test]
    fn response_ok_and_err() {
        let ok = Response::ok("hello".into());
        assert!(ok.ok);
        assert_eq!(ok.output, "hello");

        let err = Response::err("bad");
        assert!(!err.ok);
        assert_eq!(err.error, "bad");
    }
}
