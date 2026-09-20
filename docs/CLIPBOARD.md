# 图片粘贴

## 使用

Linux 从截图工具、浏览器等复制**图片内容**后，在终端按 Ctrl+Shift+V 或右键粘贴。
焦点在终端且剪贴板包含图片时，Ctrl+V 也走图片通路；其它 Ctrl+V 保持应用原有语义。

macOS 从截图或浏览器复制图片后，按 Cmd+V 或使用粘贴菜单。支持 PNG/TIFF 剪贴板内容，
后台转换成 PNG；上传期间状态栏显示蓝色上传图标，悬停可查看状态，失败显示错误。

文本粘贴不变。文件管理器复制文件路径/URI 不等于复制图片像素，本轮不做文件拖放。

前端读取图片并转成 PNG；Core 将文件存到 workspace 所在机器，再粘贴绝对路径。
只插入路径，不发送 Enter，不执行该路径，不修改远端桌面剪贴板。
Agent 是否将路径自动识别为附件取决于它的输入能力；否则可直接让它读取该路径。

SSH 目标来自 Core 打开的 workspace，不从终端内容、进程名或用户输入猜测。
因此应从 Muxterm 的 SSH 目标打开 workspace；在本地 shell 里手工嵌套 `ssh`、
容器内部再嵌套连接的文件命名空间不在自动识别范围内。

## 为什么选择文件路径

| 方案 | 文件上传后粘贴路径（本轮） | 写远端系统剪贴板 |
| --- | --- | --- |
| 无桌面 SSH | 不需要图形会话 | 依赖远端图形会话及权限 |
| 多用户、多 workspace | 由目标连接和 pane 身份隔离 | 系统剪贴板不天然属于某个 pane |
| Agent 适配 | 可以读取图片文件的 agent 可使用 | 需要 agent 的剪贴板后端和图形会话一致 |
| 安全范围 | 仅用户明确粘贴的一张图 | 还需设计跨机器剪贴板同步授权和覆盖语义 |

## Core / Transport / Frontend 分工

- Frontend：异步读取系统剪贴板，PNG 编码，显示上传状态/错误；不构造 SSH 命令。
- Core `clipboard`：检查格式/大小/尺寸、绑定原 workspace 实例和 pane、后台任务、
  bracketed paste 与路径引用、完成时验证原目标仍有效。不借用后台的 live handle。
- Transport `TargetConnection::store_temporary_file`：目标文件落地/传输。SSH 使用系统
  SSH 配置（包括已有跳板机设置）和二进制 stdin，明确 `-T` 禁止 PTY，图片不经过终端。
- C ABI：`muxterm_image_paste_start_json(handle, workspace, pane, png, len)` 复制输入后返回；
  `muxterm_image_paste_poll_json(handle)` 返回 pending 或目标 path，完成时由 Core 注入原 pane。
  调用均由 frontend event owner 串行进行。Rust CoreBridge 和公共 C 头已暴露此接口，
  Linux 与 macOS 均已接入系统剪贴板；macOS 通过主窗口命令队列与轮询调用，
  编码开始时绑定原 workspace/pane，切换页面不改变上传目的地。

## 限制与生命周期

- 一次一个图片任务，最多 20 MiB PNG、每边最多 16384 像素、总计最多 64 Mi 像素。
- 本地使用系统临时目录；SSH 使用目标 `/tmp/muxterm-paste-<随机值>.png`。
  以随机名排他创建，权限 `0600`；不安装软件，不需要远端 GUI 或 Python。
- SSH 上传上限 30 秒；失败不插入半成品路径。界面显示错误，不静默回退到另一台机器。
- 切换 workspace 不改变目标；关闭后以同 ID 重开也不会收到旧任务的输入。
- 成功文件保留给 agent 异步读取，由系统临时文件策略或用户清理。
  不在 paste 后立即删除；同用户下其它进程仍可读取，敏感截图应按需清理。
  关闭原目标后已上传成功的文件不会自动删，错误信息包含可找回的路径。

## 验证

`linux_image_paste_e2e` 使用真实 GTK 图片剪贴板、本地 shell 和隔离 loopback SSH shell：
验证图片字节、文件权限、Ctrl+V、切换 workspace 后仍粘贴到原 pane。
Core 单测验证路径引用、尺寸/类型拒绝、异步完成及关闭/重建实例保护。
`MUXTERM_IMAGE_TEST_HOST=<明确目标>` 可单独运行 `image_transport_explicit_remote_host`，
只传输随机测试文件并清理，不操作用户会话。

2026-09-19 本轮实测：GTK 本地/loopback SSH 剪贴板通过；ryzen → archmini、
archmini → ryzen 的真实二进制传输逐字节校验通过，测试文件和远端测试程序已清理。
Core 生命周期/引用测试、Transport 2 项、i18n 4 项、architecture 25 项及 Linux 构建通过。
Clippy `--lib --test linux_image_paste_e2e -D warnings` 通过；全 tests lint 被既有
`linux_render_e2e.rs` 的 `render_texture(&snapshot...)` needless borrow 阻挡，未混改该测试。
实际用户桌面上的 Ctrl+V 仍需重启新版 GUI 后验收。

官方接口核对（本轮机器时间基准 `2026-09-19T22:07:29+08:00`）：
[GDK read_texture_async](https://docs.gtk.org/gdk4/method.Clipboard.read_texture_async.html)、
[OpenSSH -T](https://man.openbsd.org/ssh.1#T)。

2026-09-20 macOS 验证：图片粘贴、命令队列与 Chrome 回归共 34 项通过；包含 TIFF 转 PNG、
无效图片拒绝、文本 bracketed paste，以及隔离 tmux 中切换 workspace 后仍向原 pane
写入图片路径并核对文件字节。Core 图片生命周期/引用测试 3 项通过。

Apple 接口核对（机器时间 `2026-09-20T10:59:22+08:00`）：
[NSPasteboard PNG](https://developer.apple.com/documentation/appkit/nspasteboard/pasteboardtype/png)、
[ImageIO 图片属性](https://developer.apple.com/documentation/imageio/cgimagesourcecopypropertiesatindex(_:_:_:))。
