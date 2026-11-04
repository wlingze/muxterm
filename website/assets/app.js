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

  // ── Screenshot carousel ────────────────────────────────────────
  const shotTrack = document.querySelector('[data-shot-track]');
  if (shotTrack) {
    const shotCards = Array.from(shotTrack.querySelectorAll('[data-shot-card]'));
    const shotPrev = document.querySelector('[data-shot-prev]');
    const shotNext = document.querySelector('[data-shot-next]');
    const shotCount = document.querySelector('[data-shot-count]');
    const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    let shotIndex = 0;

    const shotOffset = (card) => (
      shotTrack.scrollLeft
      + card.getBoundingClientRect().left
      - shotTrack.getBoundingClientRect().left
    );

    const nearestShot = () => {
      let nearest = 0;
      let distance = Infinity;
      const trackLeft = shotTrack.getBoundingClientRect().left;
      shotCards.forEach((card, index) => {
        const nextDistance = Math.abs(card.getBoundingClientRect().left - trackLeft);
        if (nextDistance < distance) {
          distance = nextDistance;
          nearest = index;
        }
      });
      return nearest;
    };

    const updateShotNav = () => {
      shotIndex = nearestShot();
      const atStart = shotTrack.scrollLeft <= 12;
      const atEnd = shotTrack.scrollLeft + shotTrack.clientWidth >= shotTrack.scrollWidth - 12;
      if (shotPrev) shotPrev.disabled = atStart;
      if (shotNext) shotNext.disabled = atEnd;
      if (shotCount) {
        shotCount.textContent = `${shotIndex + 1} / ${shotCards.length}`;
      }
    };

    const goToShot = (index) => {
      const card = shotCards[Math.max(0, Math.min(shotCards.length - 1, index))];
      if (!card) return;
      shotIndex = shotCards.indexOf(card);
      shotTrack.scrollTo({
        left: shotOffset(card),
        behavior: reduceMotion ? 'auto' : 'smooth'
      });
      updateShotNav();
    };

    if (shotPrev) shotPrev.addEventListener('click', () => goToShot(shotIndex - 1));
    if (shotNext) shotNext.addEventListener('click', () => goToShot(shotIndex + 1));
    shotTrack.addEventListener('scroll', () => {
      window.requestAnimationFrame(updateShotNav);
    }, { passive: true });
    shotTrack.addEventListener('keydown', (event) => {
      if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
      event.preventDefault();
      goToShot(shotIndex + (event.key === 'ArrowRight' ? 1 : -1));
    });
    window.addEventListener('resize', updateShotNav);
    updateShotNav();
  }

  // ── Repository docs reference ──────────────────────────────────
  const docsReference = document.querySelector('[data-docs-reference]');
  if (docsReference) {
    const docsRoot = docsReference.closest('.docs-content');
    const guideArticles = Array.from(docsRoot.querySelectorAll('.docs-article'));
    const docLinks = Array.from(document.querySelectorAll('[data-doc-ref]'));
    const guideLinks = Array.from(document.querySelectorAll('a[href^="#"]')).filter((link) => (
      link.getAttribute('href') !== '#reference' && !link.hasAttribute('data-doc-ref')
    ));
    const docTarget = docsReference.querySelector('[data-docs-content]');
    const docSource = docsReference.dataset.docsSrc || './docs/';
    const githubLink = docsReference.querySelector('[data-docs-github]');
    const docNames = new Set(docLinks.map((link) => link.dataset.docRef));
    const fallbackDoc = isZh ? 'USER-GUIDE' : 'WORKSPACE';
    let activeDoc = '';
    let requestId = 0;

    const docPath = (name) => name === 'PRODUCT' ? 'PRODUCT.md' : `docs/${name}.md`;
    const docUrl = (name) => `https://github.com/wlingze/muxterm/blob/main/${docPath(name)}`;

    const setActiveLink = (name) => {
      docLinks.forEach((link) => {
        const active = link.dataset.docRef === name;
        link.classList.toggle('is-active', active);
        if (active) {
          link.setAttribute('aria-current', 'page');
        } else {
          link.removeAttribute('aria-current');
        }
      });
      if (name && githubLink) githubLink.href = docUrl(name);
    };

    const showGuide = () => {
      docsRoot.classList.remove('is-reference-mode');
      guideArticles.forEach((article) => { article.hidden = false; });
      docsReference.hidden = true;
      activeDoc = '';
      setActiveLink('');
    };

    const showReference = () => {
      docsRoot.classList.add('is-reference-mode');
      guideArticles.forEach((article) => { article.hidden = true; });
      docsReference.hidden = false;
    };

    const sanitize = (fragment) => {
      fragment.querySelectorAll('script, style, iframe, object, embed, form, link, meta, base').forEach((node) => node.remove());
      fragment.querySelectorAll('*').forEach((node) => {
        Array.from(node.attributes).forEach((attribute) => {
          const name = attribute.name.toLowerCase();
          const value = attribute.value.trim();
          if (name.startsWith('on') || name === 'style' || name === 'srcdoc') {
            node.removeAttribute(attribute.name);
          } else if ((name === 'href' || name === 'src') && /^(?:javascript|data|vbscript):/i.test(value)) {
            node.removeAttribute(attribute.name);
          }
        });
      });
    };

    const applyHeadingIds = (root) => {
      const used = new Set();
      root.querySelectorAll('h1, h2, h3, h4').forEach((heading) => {
        const base = heading.textContent
          .trim()
          .toLowerCase()
          .replace(/[^\p{L}\p{N}\s-]/gu, '')
          .replace(/\s+/g, '-') || 'section';
        let id = `doc-${base}`;
        let suffix = 2;
        while (used.has(id)) {
          id = `doc-${base}-${suffix}`;
          suffix += 1;
        }
        used.add(id);
        heading.id = id;
      });
    };

    const absolutizeLinks = (root, name) => {
      const base = new URL(`https://github.com/wlingze/muxterm/blob/main/${docPath(name)}`);
      root.querySelectorAll('a[href]').forEach((anchor) => {
        const href = anchor.getAttribute('href');
        if (!href || href.startsWith('#') || /^(?:https?:|mailto:|tel:)/i.test(href)) return;
        const resolved = new URL(href, base);
        if (resolved.origin === 'https://github.com' && resolved.pathname.endsWith('.md')) {
          const file = resolved.pathname.split('/').pop().replace(/\.md$/, '');
          if (docNames.has(file)) {
            anchor.dataset.docLink = file;
            anchor.href = docUrl(file);
            anchor.removeAttribute('target');
            anchor.removeAttribute('rel');
            return;
          }
        }
        anchor.href = resolved.href;
        anchor.target = '_blank';
        anchor.rel = 'noopener';
      });

      root.querySelectorAll('img[src]').forEach((image) => {
        const src = image.getAttribute('src');
        if (!src || /^(?:https?:|data:)/i.test(src)) return;
        image.src = new URL(src, base.href.replace('/blob/', '/raw/')).href;
      });
    };

    const loadDoc = async (name, options = {}) => {
      if (!docNames.has(name)) return;
      const scroll = options.scroll !== false;
      const token = ++requestId;
      activeDoc = name;
      showReference();
      setActiveLink(name);
      docTarget.setAttribute('aria-busy', 'true');
      docTarget.innerHTML = `<p class="docs-reference__status">${isZh ? '加载中…' : 'loading…'}</p>`;
      history.replaceState(null, '', `#doc-${name}`);
      if (scroll) {
        window.requestAnimationFrame(() => {
          docsReference.scrollIntoView({ block: 'start', behavior: 'smooth' });
        });
      }

      try {
        if (typeof marked === 'undefined') throw new Error('markdown renderer unavailable');
        const response = await fetch(`${docSource}${name}.md`, { cache: 'no-cache' });
        if (!response.ok) throw new Error(`HTTP ${response.status}`);
        const markdown = await response.text();
        if (token !== requestId) return;

        const template = document.createElement('template');
        template.innerHTML = marked.parse(markdown, { gfm: true, breaks: false });
        sanitize(template.content);
        const title = template.content.querySelector('h1');
        if (title) {
          title.textContent = title.textContent.replace(/^[A-Z][A-Z0-9._-]*\.md\s*[—–-]\s*/, '');
        }
        docTarget.replaceChildren(template.content);
        applyHeadingIds(docTarget);
        absolutizeLinks(docTarget, name);
        docTarget.setAttribute('aria-busy', 'false');
      } catch (error) {
        if (token !== requestId) return;
        docTarget.innerHTML = `<p class="docs-reference__status">${
          isZh ? '无法加载这份文档。' : 'Could not load this document.'
        } <a href="${docUrl(name)}" target="_blank" rel="noopener">${
          isZh ? '在 GitHub 阅读' : 'read on GitHub'
        }</a></p>`;
        docTarget.setAttribute('aria-busy', 'false');
      }
    };

    docLinks.forEach((link) => {
      link.addEventListener('click', (event) => {
        event.preventDefault();
        loadDoc(link.dataset.docRef);
      });
    });

    guideLinks.forEach((link) => {
      link.addEventListener('click', () => showGuide());
    });

    docTarget.addEventListener('click', (event) => {
      const anchor = event.target.closest('a[data-doc-link]');
      if (!anchor) return;
      event.preventDefault();
      const name = anchor.dataset.docLink;
      if (name && name !== activeDoc) loadDoc(name);
    });

    const docFromHash = () => (location.hash.match(/^#doc-([A-Z0-9-]+)$/i) || [])[1];

    const initialDoc = docFromHash();
    if (initialDoc) {
      loadDoc(initialDoc, { scroll: true });
    } else if (location.hash === '#reference') {
      loadDoc(fallbackDoc, { scroll: true });
    }

    window.addEventListener('hashchange', () => {
      const name = docFromHash();
      if (name) {
        loadDoc(name, { scroll: false });
      } else if (location.hash === '#reference') {
        loadDoc(fallbackDoc, { scroll: false });
      } else {
        showGuide();
      }
    });
  }

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
