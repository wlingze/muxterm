//! CLI client：连接 daemon socket，发送 CliCommand，接收格式化输出。
//!
//! 不做单元测试（需要真实 socket + daemon），集成测试在 tests/。

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cli::ipc::{Request, Response};
use crate::cli::{CliCommand, OutputFormat};

/// 连接到 daemon socket，发送命令，返回响应。
///
/// 如果 socket 不存在或连接失败，返回 Err（调用方决定是否 fork daemon）。
pub fn send_command(
    socket_path: &Path,
    command: &CliCommand,
    format: OutputFormat,
) -> Result<Response> {
    let stream = UnixStream::connect(socket_path)
        .with_context(|| format!("连接 daemon 失败: {}", socket_path.display()))?;

    let req = Request {
        command: command.clone(),
        format,
    };
    let req_json = serde_json::to_string(&req).context("序列化请求失败")?;

    {
        let mut writer = &stream;
        writeln!(writer, "{req_json}").context("发送请求失败")?;
        writer.flush().context("flush 失败")?;
    }

    let reader = BufReader::new(&stream);
    let mut last_response: Option<Response> = None;

    for line in reader.lines() {
        let line = line.context("读取响应行")?;
        if line.is_empty() {
            continue;
        }
        let resp: Response = serde_json::from_str(&line).context("反序列化响应失败")?;
        last_response = Some(resp);
        break; // 只取第一个响应
    }

    last_response.context("未收到 daemon 响应")
}
