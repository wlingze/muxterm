//! TUI 事件循环入口。
//!
//! `run()` 进入 crossterm raw mode + alternate screen，构造 `TerminalModel`（
//! LocalBackend 或 TmuxBackend），轮询键盘事件 → `Task::SendKeys`，轮询
//! `StateChange` → 重绘。Ctrl-Q / Ctrl-D 退出。
//!
//! 不做单元测试（需要真实终端 I/O）；渲染逻辑在 `render.rs` 单测。
//!
//! 事件循环本身是同步的（crossterm::read 阻塞），但 backend connect/shutdown
//! 是 async（用 tokio spawn 子进程）。这里用 `tokio::runtime::Runtime` 把
//! async 调用 block_on，整个 run 包在一个轻量 runtime 里。

use std::io::{stdout, Write};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{poll, read, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};

use crate::core::backend::{LocalBackend, TmuxBackend};
use crate::core::model::state::State;
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::model::TerminalModel;
use crate::core::terminal::input::{ArrowDir, KeyEvent as MuxKeyEvent};
use crate::platform::tui::render::{render_frame, RenderOpts};

/// 启动 TUI 前端。
///
/// `socket` 对应 CLI `-L/--socket`：非空时用 TmuxBackend，否则用 LocalBackend。
pub fn run(socket: Option<String>) -> Result<()> {
    // 进入 raw mode + alternate screen
    enable_raw_mode().context("enable raw mode")?;
    let mut out = stdout();
    let result = run_inner(&mut out, socket);
    // 恢复终端（无论如何都执行）
    let _ = disable_raw_mode();
    let _ = execute!(out, Show, LeaveAlternateScreen);
    result
}

fn run_inner(out: &mut impl Write, socket: Option<String>) -> Result<()> {
    execute!(out, EnterAlternateScreen, Hide, Clear(ClearType::All))
        .context("enter alternate screen")?;

    // 用一个轻量 tokio runtime 执行 async backend connect/shutdown
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;

    // 构造 backend + model
    let backend: Box<dyn crate::core::model::Backend> = if let Some(ref sock) = socket {
        let s = sock.trim();
        if s.is_empty() {
            Box::new(LocalBackend::new("$SHELL", ""))
        } else {
            Box::new(TmuxBackend::new(Some(s)))
        }
    } else {
        Box::new(LocalBackend::new("$SHELL", ""))
    };
    let mut model = TerminalModel::new(backend);

    // connect（spawn 默认 shell / tmux）
    rt.block_on(model.connect())?;
    // drain 启动事件
    let _ = model.poll_events();

    let (cols, rows) = size().unwrap_or((80, 24));
    let mut opts = RenderOpts {
        cols,
        rows,
        max_output_lines: 8,
    };

    redraw(out, model.state(), opts)?;

    // 事件循环
    loop {
        // 先 drain 状态变更事件（pty 输出等），有变化则重绘
        let events = model.poll_events();
        if !events.is_empty() {
            if let Ok((c, r)) = size() {
                opts.cols = c;
                opts.rows = r;
            }
            redraw(out, model.state(), opts)?;
        }

        // 轮询键盘事件（100ms 超时，让 pty 输出有机会被 drain）
        if poll(Duration::from_millis(100)).context("poll event")? {
            let ev = read().context("read event")?;
            match ev {
                Event::Key(key) => {
                    if is_quit(&key) {
                        break;
                    }
                    if let Some(task) = key_event_to_task(&key, model.state()) {
                        let outcome = model.execute(task)?;
                        if matches!(outcome, TaskOutcome::Rejected { .. }) {
                            // 忽略 reject（比如无 active pane）
                        }
                        redraw(out, model.state(), opts)?;
                    }
                }
                Event::Resize(c, r) => {
                    opts.cols = c;
                    opts.rows = r;
                    redraw(out, model.state(), opts)?;
                }
                _ => {}
            }
        }
    }

    // 关闭后端
    let _ = rt.block_on(model.shutdown());
    Ok(())
}

/// 重绘一帧：清屏 + 移到左上 + 输出渲染行。
fn redraw(out: &mut impl Write, state: &dyn State, opts: RenderOpts) -> Result<()> {
    queue!(out, crossterm::cursor::MoveTo(0, 0), Clear(ClearType::All)).context("clear")?;
    let lines = render_frame(state, opts);
    for (i, line) in lines.iter().enumerate() {
        queue!(out, crossterm::cursor::MoveTo(0, i as u16)).context("move cursor")?;
        out.write_all(line.as_bytes()).context("write line")?;
    }
    out.flush().context("flush")?;
    Ok(())
}

/// Ctrl-Q / Ctrl-D / Ctrl-C 退出。
fn is_quit(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(
            key.code,
            KeyCode::Char('q') | KeyCode::Char('d') | KeyCode::Char('c')
        )
}

/// 把 crossterm KeyEvent 转成 muxterm Task。
///
/// - Alt+n → new window
/// - Alt+字符 → 发 ESC 前缀
/// - Ctrl+字符 → Ctrl 组合键
/// - 方向键 / 普通字符 → 对应 KeyEvent
fn key_event_to_task(key: &KeyEvent, state: &dyn State) -> Option<Task> {
    let target = state.active_pane().map(|p| p.id)?;

    // Alt 组合
    if key.modifiers.contains(KeyModifiers::ALT) {
        if let KeyCode::Char(c) = key.code {
            return Some(match c {
                'n' => Task::NewWindow {
                    name: None,
                    command: None,
                    workdir: None,
                },
                _ => {
                    return Some(Task::SendKeys {
                        target,
                        keys: vec![MuxKeyEvent::Alt(c)],
                    });
                }
            });
        }
    }

    // Ctrl 组合（Ctrl-Q/D/C 已在 is_quit 处理，这里其余 Ctrl+字符）
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            return Some(Task::SendKeys {
                target,
                keys: vec![MuxKeyEvent::Ctrl(c)],
            });
        }
    }

    // 方向键 / 普通键
    let task_key = match key.code {
        KeyCode::Char(c) => MuxKeyEvent::Char(c),
        KeyCode::Enter => MuxKeyEvent::Enter,
        KeyCode::Tab => MuxKeyEvent::Tab,
        KeyCode::BackTab => MuxKeyEvent::Tab,
        KeyCode::Backspace => MuxKeyEvent::Backspace,
        KeyCode::Esc => MuxKeyEvent::Escape,
        KeyCode::Up => MuxKeyEvent::Arrow(ArrowDir::Up),
        KeyCode::Down => MuxKeyEvent::Arrow(ArrowDir::Down),
        KeyCode::Left => MuxKeyEvent::Arrow(ArrowDir::Left),
        KeyCode::Right => MuxKeyEvent::Arrow(ArrowDir::Right),
        KeyCode::F(n) => MuxKeyEvent::Function(n),
        _ => return None,
    };
    Some(Task::SendKeys {
        target,
        keys: vec![task_key],
    })
}
