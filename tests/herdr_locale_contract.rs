//! 新 workspace / tab / split 的真实中文回显；server 故意继承 ASCII locale。
mod support;
use muxterm::test_support::core::{
    protocol::{
        layout::SplitDir,
        state::StateChange,
        task::{Task, TaskOutcome},
        PaneId, WorkspaceId,
    },
    runtime::herdr::{HerdrRuntime, HerdrSession},
    transport::connection::Connect,
    workspace::Workspace,
};
use std::{sync::Arc, time::Instant};
use support::herdr_test_support::{
    herdr_available, seed_herdr_pane_viewport, seed_herdr_viewport, IsolatedHerdr,
};
use support::sshd_test_support::LoopbackSshd;
const HERDR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[test]
fn created_herdr_pane_uses_utf8_even_when_server_started_without_it() -> anyhow::Result<()> {
    if !herdr_available() || !std::path::Path::new("/bin/zsh").exists() {
        return Ok(());
    }
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(
        std::env::temp_dir().join(format!("muxterm-test-herdr-locale-{}", std::process::id())),
    );
    std::fs::create_dir_all(&scratch.0)?;
    std::fs::write(
        scratch.0.join(".zshenv"),
        "unsetopt rcs\npsvar[1]='⇣⇡'\nPROMPT='MUXINPUT> %1v '\nRPROMPT=''\n",
    )?;
    let herdr = IsolatedHerdr::start_with_environment(
        "input-locale",
        &[
            ("LANG".into(), "C".into()),
            ("LC_ALL".into(), "C".into()),
            ("LC_CTYPE".into(), "C".into()),
            ("SHELL".into(), "/bin/zsh".into()),
            ("ZDOTDIR".into(), scratch.0.to_string_lossy().into_owned()),
        ],
    );
    let sshd = LoopbackSshd::start("herdr-locale")?;
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(value) = &self.0 {
                std::env::set_var("MUXTERM_SSH_CONFIG_PATH", value);
            } else {
                std::env::remove_var("MUXTERM_SSH_CONFIG_PATH");
            }
        }
    }
    let _restore = Restore(std::env::var_os("MUXTERM_SSH_CONFIG_PATH"));
    sshd.apply_ssh_config_env();
    for (transport, target) in [("local", ""), ("ssh", sshd.alias.as_str())] {
        let session = Arc::new(HerdrSession::with_connection(
            Connect::new(transport, target),
            herdr.name(),
            herdr.socket_path(),
        ));
        check_created_panes(&herdr, session, transport, target)?;
    }
    Ok(())
}

fn check_created_panes(
    herdr: &IsolatedHerdr,
    session: Arc<HerdrSession>,
    transport: &str,
    target: &str,
) -> anyhow::Result<()> {
    let created = session.workspace_create("/tmp", "utf8-input")?;
    let wire_pane = session
        .snapshot()?
        .panes
        .into_iter()
        .find(|pane| pane.workspace_id == created.workspace_id)
        .expect("created pane")
        .pane_id;
    let runtime = HerdrRuntime::new(Arc::clone(&session), &created.workspace_id);
    let mut workspace = Workspace::new(
        WorkspaceId::new(
            transport,
            Some(target),
            herdr.name(),
            "herdr",
            &created.workspace_id,
        ),
        "utf8-input".into(),
        Box::new(runtime),
    );
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(workspace.connect())?;
    seed_herdr_viewport(&mut workspace, 100, 30)?;
    let initial = workspace.state().active_pane().expect("active pane").id;
    assert_utf8_echo(herdr, &session, &mut workspace, initial, &wire_pane)?;
    for split in [false, true] {
        let previous_wire: Vec<_> = session
            .snapshot()?
            .panes
            .into_iter()
            .filter(|pane| pane.workspace_id == created.workspace_id)
            .map(|pane| pane.pane_id)
            .collect();
        let previous_products = product_panes(&workspace);
        let task = if split {
            Task::SplitPane {
                target: Some(workspace.state().active_pane().expect("active pane").id),
                dir: SplitDir::Vertical,
                command: None,
                workdir: None,
            }
        } else {
            Task::NewTab {
                name: None,
                command: None,
                workdir: None,
            }
        };
        workspace.execute(task)?;
        let deadline = Instant::now() + HERDR_TIMEOUT;
        let (wire, product) = loop {
            workspace.refresh();
            if let Some(pane) = session.snapshot()?.panes.into_iter().find(|pane| {
                pane.workspace_id == created.workspace_id && !previous_wire.contains(&pane.pane_id)
            }) {
                let products = product_panes(&workspace);
                if let Some(product) = products
                    .iter()
                    .copied()
                    .find(|id| !previous_products.contains(id))
                {
                    break (pane.pane_id, product);
                }
            }
            anyhow::ensure!(
                Instant::now() < deadline,
                "new pane topology did not converge"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        // 只给新 pane 开流。seed 到仍然 focused 的旧 pane 会让新 pane 停在 Absent。
        seed_herdr_pane_viewport(&mut workspace, product, 100, 30)?;
        assert_utf8_echo(herdr, &session, &mut workspace, product, &wire)?;
    }
    Ok(())
}

fn product_panes(workspace: &Workspace) -> Vec<PaneId> {
    workspace
        .state()
        .tabs()
        .iter()
        .flat_map(|tab| workspace.state().panes(&tab.id))
        .map(|pane| pane.id)
        .collect()
}

fn assert_utf8_echo(
    herdr: &IsolatedHerdr,
    session: &HerdrSession,
    workspace: &mut Workspace,
    pane: PaneId,
    wire_pane: &str,
) -> anyhow::Result<()> {
    herdr.wait_for_pane_process(wire_pane);
    // pane.read 能看到提示符时，control stream 还没有第一帧。这时 WriteRaw 只会
    // 进队列，慢 runner 上 3 秒内看不到回显。先等这个 pane 的 live frame。
    let deadline = Instant::now() + HERDR_TIMEOUT;
    let mut prompt_ready = false;
    let mut stream_ready = false;
    loop {
        let events = workspace.refresh();
        if events.iter().any(
            |event| matches!(event, StateChange::PaneFrame { pane: frame, .. } if *frame == pane),
        ) {
            stream_ready = true;
        }
        let prompt = String::from_utf8_lossy(&session.pane_read_ansi(wire_pane)?).into_owned();
        if prompt.contains("MUXINPUT>") && prompt.contains("⇣⇡") {
            prompt_ready = true;
        }
        if prompt_ready && stream_ready {
            break;
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "UTF-8 prompt or control frame was not ready on {wire_pane}: prompt={prompt_ready} stream={stream_ready} screen={prompt:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let outcome = workspace.execute(Task::WriteRaw {
        target: pane,
        data: "中文测试".as_bytes().to_vec(),
    })?;
    anyhow::ensure!(
        outcome == TaskOutcome::Done,
        "WriteRaw to {pane} via {wire_pane} was not delivered: {outcome:?}"
    );
    let deadline = Instant::now() + HERDR_TIMEOUT;
    loop {
        workspace.refresh();
        let seen = String::from_utf8_lossy(&session.pane_read_ansi(wire_pane)?).into_owned();
        if seen.contains("中文测试") {
            return Ok(());
        }
        anyhow::ensure!(
            Instant::now() < deadline,
            "Chinese input was not echoed as UTF-8 on {wire_pane}: {seen:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
