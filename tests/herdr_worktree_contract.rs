//! H4：worktree list / create / open（只在临时 git 仓库上测）。
//!
//! 夹具路径只许 `/tmp/muxterm-test-herdr-*`；禁止在本仓库 `git worktree add`。

mod support;

use muxterm::core::model::backend::WorktreeCreateSpec;
use muxterm::core::types::TabId;
use muxterm::core::workspace::pool::{WorkspacePool, WorkspacePoolPolicy};
use muxterm::core::workspace::spec::WorkspaceSpec;
use support::herdr_test_support::{herdr_available, IsolatedHerdr, TempGitRepo};

/// 同一测试里 list/create/open 全走一遍。
#[test]
fn herdr_worktree_contract() {
    if !herdr_available() {
        eprintln!("skip: 无 herdr 二进制");
        return;
    }
    let herdr = IsolatedHerdr::start("wt");
    let mut repo = TempGitRepo::new("wt");
    let repo_path = repo.path().to_string_lossy().to_string();
    let (ws, _tab, _pane) = herdr.create_workspace(&repo_path, "mux-wt");

    let mut pool = WorkspacePool::new(WorkspacePoolPolicy::new(8));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .expect("tokio");
    let socket = herdr.socket_path().to_string_lossy().to_string();
    let spec = WorkspaceSpec::herdr(herdr.name(), ws.clone(), socket.clone());
    let id = spec.id();
    rt.block_on(pool.open_spec(&spec))
        .expect("open 主 checkout 失败");

    // 1. list：至少一行主 checkout；path 是 temp repo；open_workspace 对得上当前格。
    let list = rt.block_on(pool.list_worktrees(&id)).expect("list 应成功");
    assert!(!list.is_empty(), "list 至少一行主 checkout");
    let main = list.iter().find(|w| !w.linked).expect("主 checkout");
    assert_eq!(main.path, repo_path, "主 checkout path 是 temp repo");
    assert_eq!(
        main.open_workspace,
        Some(id.clone()),
        "主 checkout 对得上当前格"
    );

    // /tmp 非 git 目录：list 应失败或空，且不得 panic。
    let (tmp_ws, _tt, _tp) = herdr.create_workspace("/tmp", "mux-wt-tmp");
    let tmp_spec = WorkspaceSpec::herdr(herdr.name(), tmp_ws.clone(), socket.clone());
    let tmp_id = tmp_spec.id();
    rt.block_on(pool.open_spec(&tmp_spec))
        .expect("open /tmp 工作区失败");
    if let Ok(list) = rt.block_on(pool.list_worktrees(&tmp_id)) {
        assert!(list.is_empty(), "/tmp 非 git 目录 list 应为空");
    }

    // 2. create：--branch muxterm-test-wt-* --path /tmp/muxterm-test-herdr-wt-* --no-focus。
    let branch = repo.unique_branch("c1");
    let wt_path = repo.unique_worktree_path("c1");
    let create_spec = WorktreeCreateSpec {
        branch: branch.clone(),
        path: wt_path.to_string_lossy().to_string(),
        base: None,
        label: None,
    };
    let new_id = rt
        .block_on(pool.create_worktree(&id, &create_spec))
        .expect("worktree.create 应成功");
    repo.track_worktree(&wt_path);
    assert_ne!(new_id, id, "新格是另一格");
    assert!(pool.get(&new_id).is_some(), "池里多一格");

    let list2 = rt.block_on(pool.list_worktrees(&id)).expect("list2 应成功");
    let linked = list2
        .iter()
        .find(|w| w.linked)
        .expect("list 能看到 is_linked_worktree");
    assert_eq!(
        linked.path,
        wt_path.to_string_lossy(),
        "新格 path 是新 checkout"
    );
    assert_eq!(
        linked.open_workspace,
        Some(new_id.clone()),
        "linked checkout 对得上新格"
    );
    let new_ws = pool.get(&new_id).expect("新格在池里");
    let wt_basename = wt_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let tabs: Vec<String> = new_ws
        .state()
        .tabs()
        .iter()
        .map(|t| format!("{}:{}", t.id, t.name))
        .collect();
    let all_panes: Vec<String> = new_ws
        .state()
        .tabs()
        .iter()
        .flat_map(|t| new_ws.state().panes(&t.id))
        .map(|p| format!("{}:{}", p.id, p.title))
        .collect();
    let titles: Vec<String> = new_ws
        .state()
        .panes(&TabId(1))
        .iter()
        .map(|p| p.title.clone())
        .collect();
    assert!(
        titles.iter().any(|t| t.contains(&wt_basename)),
        "新格 pane cwd/title 是新 checkout（{wt_basename}），tabs={tabs:?} all_panes={all_panes:?} titles={titles:?}"
    );

    // 3. open：对已存在 path 再 open，返回已有 WorkspaceId，不复制一格。
    let opened_id = rt
        .block_on(pool.open_worktree(&id, &wt_path.to_string_lossy()))
        .expect("worktree.open 应成功");
    assert_eq!(opened_id, new_id, "open 已存在 checkout 返回同一格");
    assert_eq!(pool.len(), 3, "不复制一格（主 + linked + /tmp）");
    pool.shutdown_all();
}
