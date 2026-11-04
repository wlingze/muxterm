/**
 * Muxterm official website client-side interaction.
 * Zero external dependencies.
 */

document.addEventListener('DOMContentLoaded', () => {
  const isZh = document.documentElement.lang.toLowerCase().startsWith('zh');
  const root = document.documentElement;

  // ── Theme: black / white ────────────────────────────────────────
  const themeToggles = document.querySelectorAll('[data-theme-toggle]');
  const themeButtons = document.querySelectorAll('[data-set-theme]');
  const shotImages = document.querySelectorAll('[data-shot]');

  const applyTheme = (theme) => {
    const next = theme === 'light' ? 'light' : 'dark';
    root.dataset.theme = next;

    themeToggles.forEach((toggle) => {
      const label = isZh
        ? (next === 'light' ? '黑色' : '白色')
        : (next === 'light' ? 'black' : 'white');
      toggle.textContent = label;
      toggle.setAttribute('aria-label', isZh
        ? (next === 'light' ? '切换到黑色主题' : '切换到白色主题')
        : (next === 'light' ? 'Switch to black theme' : 'Switch to white theme'));
      toggle.setAttribute('aria-pressed', next === 'light' ? 'true' : 'false');
    });

    themeButtons.forEach((button) => {
      button.setAttribute('aria-pressed', button.dataset.setTheme === next ? 'true' : 'false');
    });

    shotImages.forEach((image) => {
      const source = next === 'light' ? image.dataset.light : image.dataset.dark;
      if (source && image.getAttribute('src') !== source) {
        image.src = source;
      }
      const link = image.closest('a.scene__open');
      if (link && source) {
        link.href = source;
      }
    });
  };

  const setTheme = (next) => {
    applyTheme(next);
    try {
      localStorage.setItem('muxterm-theme', next);
    } catch {
      // Storage can be unavailable in private or restricted contexts.
    }
  };

  applyTheme(root.dataset.theme || 'dark');

  themeToggles.forEach((toggle) => {
    toggle.addEventListener('click', () => {
      setTheme(root.dataset.theme === 'light' ? 'dark' : 'light');
    });
  });

  themeButtons.forEach((button) => {
    button.addEventListener('click', () => {
      setTheme(button.dataset.setTheme === 'light' ? 'light' : 'dark');
    });
  });

  // ── Install snippet switcher and copy ───────────────────────────
  const installTabs = document.querySelectorAll('.install-tab');
  const installCommand = document.querySelector('.install-command');
  const copyBtn = document.querySelector('.install-copy-btn');

  const commands = {
    macos: 'curl -L -o muxterm.dmg https://github.com/wlingze/muxterm/releases/latest/download/muxterm-macos-arm64.dmg',
    linux: 'curl -L -o muxterm.tar.gz https://github.com/wlingze/muxterm/releases/latest/download/muxterm-gtk-linux-x86_64.tar.gz',
    source: 'git clone https://github.com/wlingze/muxterm.git'
  };

  installTabs.forEach((tab) => {
    tab.addEventListener('click', () => {
      installTabs.forEach((t) => {
        t.classList.remove('is-active');
        t.setAttribute('aria-pressed', 'false');
      });
      tab.classList.add('is-active');
      tab.setAttribute('aria-pressed', 'true');
      const key = tab.dataset.target || 'macos';
      if (installCommand && commands[key]) {
        installCommand.textContent = '$ ' + commands[key];
      }
    });
  });

  if (copyBtn && installCommand) {
    const originalLabel = copyBtn.textContent;
    copyBtn.addEventListener('click', async () => {
      const text = installCommand.textContent.replace(/^\$\s*/, '').trim();
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        const textarea = document.createElement('textarea');
        textarea.value = text;
        document.body.appendChild(textarea);
        textarea.select();
        document.execCommand('copy');
        textarea.remove();
      }
      copyBtn.textContent = isZh ? '已复制' : 'copied';
      setTimeout(() => {
        copyBtn.textContent = originalLabel;
      }, 1400);
    });
  }

  // ── Port forwarding demo ────────────────────────────────────────
  document.querySelectorAll('[data-port]').forEach((button) => {
    const row = button.closest('.port-row');
    const local = row ? row.querySelector('.port-local') : null;
    button.addEventListener('click', () => {
      const on = button.getAttribute('aria-pressed') === 'true';
      button.setAttribute('aria-pressed', on ? 'false' : 'true');
      button.classList.toggle('is-on', !on);
      button.textContent = on
        ? (isZh ? '转发' : 'forward')
        : (isZh ? '停止' : 'stop');
      if (local) {
        local.textContent = on ? '' : `localhost:${button.dataset.port}`;
      }
    });
  });

  // ── Interactive Workspace / Tab / Pane surface ──────────────────
  const workspaces = document.querySelectorAll('.workspace-item');
  const tabs = document.querySelectorAll('.surface-tab');
  const leftPane = document.getElementById('pane-left');
  const rightPane = document.getElementById('pane-right');
  const statusWorkspace = document.getElementById('status-workspace-name');

  const englishData = {
    'ws-local': {
      name: 'muxterm @ local (tmux)',
      tabs: ['build', 'editor', 'logs'],
      panes: {
        left: `$ cargo check --features gtk\n    Checking muxterm\n    Finished dev profile\n\n$ muxterm list-workspaces\n[1] muxterm @ local (tmux)\n[2] yaklang @ remote (herdr)\n[3] staging-node @ aws (ssh+tmux)`,
        right: `$ git status\nOn branch main\nnothing to commit, working tree clean\n\nRuntime: tmux\nTransport: local\nScene: resident`
      }
    },
    'ws-herdr': {
      name: 'yaklang @ remote (herdr)',
      tabs: ['server', 'agent', 'terminal'],
      panes: {
        left: `$ herdr workspace status\nworkspace: yaklang\nsocket: herdr.sock\npanes: server, agent\n\nAgent: working\nWorktree: feature/parser`,
        right: `$ git status\nOn branch feature/parser\nnothing to commit, working tree clean\n\nRuntime: herdr\nTransport: ssh\nScene: resident`
      }
    },
    'ws-ssh': {
      name: 'staging-node @ aws (ssh+tmux)',
      tabs: ['cluster', 'ports'],
      panes: {
        left: `$ ssh aws-staging -CC attach -t staging\nByteChannel multiplexed over SSH\nTargetConnection reused\nAttached session: staging`,
        right: `$ ss -ltn\nLISTEN 127.0.0.1:3000\nLISTEN 127.0.0.1:5432\n\nPorts detected\nForward: localhost:3000`
      }
    }
  };

  const chineseData = {
    'ws-local': {
      name: 'muxterm · 本地 tmux',
      tabs: ['构建', '编辑', '日志'],
      panes: {
        left: `$ cargo check --features gtk\n    Checking muxterm\n    Finished dev profile\n\n$ muxterm list-workspaces\n[1] muxterm @ local (tmux)\n[2] yaklang @ remote (herdr)\n[3] staging-node @ aws (ssh+tmux)`,
        right: `$ git status\nOn branch main\nnothing to commit, working tree clean\n\nRuntime: tmux\nTransport: local\nScene: resident`
      }
    },
    'ws-herdr': {
      name: 'yaklang · 远程 herdr',
      tabs: ['服务', 'Agent', '终端'],
      panes: {
        left: `$ herdr workspace status\nworkspace: yaklang\nsocket: herdr.sock\npanes: server, agent\n\nAgent: 正在工作\nWorktree: feature/parser`,
        right: `$ git status\nOn branch feature/parser\nnothing to commit, working tree clean\n\nRuntime: herdr\nTransport: ssh\nScene: resident`
      }
    },
    'ws-ssh': {
      name: 'staging-node · AWS ssh+tmux',
      tabs: ['集群', '端口'],
      panes: {
        left: `$ ssh aws-staging -CC attach -t staging\nByteChannel 经 SSH 复用\nTargetConnection 复用\n已连接会话: staging`,
        right: `$ ss -ltn\nLISTEN 127.0.0.1:3000\nLISTEN 127.0.0.1:5432\n\n已发现端口\n转发: localhost:3000`
      }
    }
  };

  const workspaceData = isZh ? chineseData : englishData;

  workspaces.forEach((ws) => {
    ws.addEventListener('click', () => {
      workspaces.forEach((w) => {
        w.classList.remove('is-active');
        w.setAttribute('aria-pressed', 'false');
      });
      ws.classList.add('is-active');
      ws.setAttribute('aria-pressed', 'true');
      const data = workspaceData[ws.dataset.ws];
      if (!data) return;

      if (statusWorkspace) {
        statusWorkspace.textContent = data.name;
      }

      tabs.forEach((tab, index) => {
        if (data.tabs[index]) {
          tab.textContent = data.tabs[index];
          tab.style.display = 'inline-block';
        } else {
          tab.style.display = 'none';
        }
        tab.classList.toggle('is-active', index === 0);
        tab.setAttribute('aria-pressed', index === 0 ? 'true' : 'false');
      });

      if (leftPane) leftPane.textContent = data.panes.left;
      if (rightPane) rightPane.textContent = data.panes.right;
    });
  });

  tabs.forEach((tab) => {
    tab.addEventListener('click', () => {
      tabs.forEach((t) => {
        t.classList.remove('is-active');
        t.setAttribute('aria-pressed', 'false');
      });
      tab.classList.add('is-active');
      tab.setAttribute('aria-pressed', 'true');
    });
  });
});
