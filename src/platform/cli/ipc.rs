//! IPC 协议：CLI client ↔ daemon 之间的消息格式。
//!
//! 用 serde_json over unix socket 通信（每条消息一行 JSON，以 `\n` 分隔）。
//! CliCommand 和 OutputFormat 直接 derive serde，用 tagged enum 序列化。

pub use crate::core::protocol::daemon::{Request, Response};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::protocol::command::CliCommand;
    use crate::core::protocol::daemon::OutputFormat;
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
