//! TUI 事件循环入口。
//!
//! `run()` 进入 crossterm raw mode + alternate screen，构造 `TerminalModel`（
//! LocalBackend 或 TmuxBackend），轮询键盘事件 → `Task`，轮询 `StateChange`
//! → 重绘。Ctrl-Q 退出，Alt+T 新建 tab，Alt+数字切 tab。
//!
//! 不做单元测试（需要真实终端 I/O）；渲染逻辑在 `render.rs` 单测，
//! 集成测试在 `tests/` 目录（spawn TUI 进程 + tmux capture）。

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
use crate::core::model::layout::SplitDir;
use crate::core::model::state::State;
use crate::core::model::task::{Task, TaskOutcome};
use crate::core::model::TerminalModel;
use crate::core::terminal::input::{ArrowDir, KeyEvent as MuxKeyEvent};
use crate::core::types::WindowId;
use crate::platform::tui::render::{render_frame, RenderOpts};

/// 启动 TUI 前端。
///
/// `socket` 对应 CLI `-L/--socket`：非空时用 TmuxBackend，否则用 LocalBackend。
pub fn run(socket: Option<String>) -> Result<()> {
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
    let _ = model.poll_events();

    let (cols, rows) = size().unwrap_or((80, 24));
    let mut opts = RenderOpts {
        cols,
        rows,
        max_output_lines: 20,
    };

    redraw(out, model.state(), opts)?;

    // 事件循环
    loop {
        // drain 状态变更事件（pty 输出等），有变化则重绘。
        // 用 refresh() 而非 poll_events()：refresh 会先从 backend 拉取
        // 最新 pty 输出（execute 之后 shell 产生的回显/命令输出），否则
        // 这些输出会一直堆积在 backend 缓冲里，TUI 永远看不到。
        let events = model.refresh();
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
                            // 忽略 reject
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

    let _ = rt.block_on(model.shutdown());
    Ok(())
}

/// 重绘一帧。
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
/// - Alt+T → new window（新建 tab）
/// - Alt+1..9 → switch window（切 tab）
/// - Alt+字符 → 发 ESC 前缀
/// - Ctrl+字符 → Ctrl 组合键
/// - 方向键 / 普通字符 → 对应 KeyEvent
fn key_event_to_task(key: &KeyEvent, state: &dyn State) -> Option<Task> {
    let target = state.active_pane().map(|p| p.id);

    // Alt 组合
    if key.modifiers.contains(KeyModifiers::ALT) {
        if let KeyCode::Char(c) = key.code {
            let lower = c.to_ascii_lowercase();
            return Some(match lower {
                't' => Task::NewWindow {
                    name: None,
                    command: None,
                    workdir: None,
                },
                '1' | '2' | '3' | '4' | '5' | '6' | '7' | '8' | '9' => {
                    // Alt+N → 第 N 个 window（0-based: Alt+1=@0, Alt+2=@1...）
                    let n = lower.to_digit(10).unwrap();
                    Task::SwitchWindow {
                        target: WindowId(n.saturating_sub(1)),
                    }
                }
                // Alt+W 关闭当前 window
                'w' => Task::CloseWindow {
                    target: state.active_window()?.id,
                },
                // Alt+S 左右分割（水平分割）
                's' => Task::SplitPane {
                    target,
                    dir: SplitDir::Horizontal,
                    command: None,
                    workdir: None,
                },
                // Alt+V 上下分割（垂直分割）
                'v' => Task::SplitPane {
                    target,
                    dir: SplitDir::Vertical,
                    command: None,
                    workdir: None,
                },
                _ => {
                    let target = target?;
                    return Some(Task::SendKeys {
                        target,
                        keys: vec![MuxKeyEvent::Alt(c)],
                    });
                }
            });
        }
    }

    // Ctrl 组合（Ctrl-Q/D/C 已在 is_quit 处理）
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            let target = target?;
            return Some(Task::SendKeys {
                target,
                keys: vec![MuxKeyEvent::Ctrl(c)],
            });
        }
    }

    // 方向键 / 普通键
    let target = target?;
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
