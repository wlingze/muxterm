# 终端能力取舍：图片、HTML、libghostty、远程打开与扩展

> 更新：2026-10-29（Asia/Shanghai）。这是调研与路线补充，不是实现完成记录。
> 机器时间核对：2026-10-29T14:46:40+08:00、2026-10-29T14:52:17+08:00、
> 2026-10-29T15:11:51+08:00、2026-10-29T15:17:29+08:00；OpenSSH 补充核对截至 2026-10-29T15:23:10+08:00。
> 官方资料在上述时间区间直接联网读取；没有使用交互浏览器，也没有运行竞品性能测试。
> 产品优先级见 [`PRODUCT-DIRECTION.md`](PRODUCT-DIRECTION.md)，输出阅读见
> [`OUTPUT-READING.md`](OUTPUT-READING.md)。本轮不迁移终端内核、不实现插件平台。

## 1. 结论

图片显示、富文本复制、远程文件打开和端口转发都能改善普通终端的输入与结果查看，
不需要先接入 ACP。继续完成这些交互，再进入以可靠性和兼容性为主的稳定维护是合理路线。

**当前保留 SwiftTerm / VTE；libghostty-vt 值得做有边界的对照验证，尚无立即迁移依据。**
先验证现有图片能力与复用器链路。扩展先采用可配置命令和外部打开工具，
文件管理器可以按需在目标机器的终端中运行；暂不建设通用插件 SDK、任意 Web 面板或市场。

## 2. Ghostty 最近到底增加了什么

官方发布索引最新列出 Ghostty **1.4.0，2026-10-27 发布** [1–2]。

| 能力 | 核实结果 | 对 Muxterm 的含义 |
| --- | --- | --- |
| 终端内联图片 | 1.4 增加 Kitty graphics 动画、相对定位；上游宣称覆盖全部已发布规范 [2] | 值得对照实际图片样例；不把上游覆盖率当成 Muxterm 的兼容性或性能测量 |
| HTML 剪贴板 | 实现 Kitty Clipboard Protocol（OSC 5522），允许传递图片、HTML 等 MIME 内容 [2, 7] | 这是程序与剪贴板的数据交换，需要应用及中间链路配合 |
| HTML 导出/复制 | 终端内容 formatter 支持 plain / VT / HTML，1.4 继续改进 [2–3] | 可服务保留颜色的复制和输出导出；不是本次才从零引入 HTML |
| 网页渲染 | 本次公告与 API 没有为终端内运行 HTML/DOM/JavaScript 浏览器提供依据 | 不因“支持 HTML”就规划内嵌浏览器 |
| tmux control mode | 发布说明有修复，但明确尚未接入 Ghostty GUI [2] | 库有协议能力不等于已经替代 Muxterm 的复用器操作体验 |

因此应把“HTML”拆成具体需求：保留样式复制、打开生成的 HTML 报告、在应用里嵌网页。
前两项可以独立交付。远程 HTML 报告优先通过其 HTTP 服务和端口转发交给外部浏览器；
只下载单个 HTML 文件不能作为带相对资源报告的完整验收。独立网页面板尚无必做依据。

OSC 5522 也不替代现有图片上传：终端支持该协议，不代表每个 agent 都会请求图片 MIME。
继续保留“图片放到正确机器 → 插入路径”的已用流程，协议增强按实际程序支持情况补充。

## 3. libghostty：有吸引力，但不是可直接替换的终端控件

### 两套接口必须分清

| 接口 | 提供什么 | Muxterm 需要承担什么 |
| --- | --- | --- |
| 公开 `libghostty-vt` | VT 解析、屏幕/回滚状态、reflow、输入编码、图片状态、搜索、formatter、快照及供自绘使用的 render state [3] | 绘制、字体与文本布局、原生输入/IME、选择交互、滚动、无障碍和平台集成 |
| `include/ghostty.h` 内部 embedder API | 为 Ghostty macOS 应用设计的更完整接口 | 头文件明确说不面向外部使用，含 macOS/Metal 专用部分；采用它意味着自行承担内部 API 集成维护 [4] |

官方 Ghostling 示例用 Raylib 自己绘制，明确说明 libghostty-vt **不含绘制或窗口代码** [5]。
接上 C API 并不会自动获得 Ghostty 的 Metal renderer 或完整原生终端 widget。
使用内部接口或移植上游绘制代码并非不可能，但不能把它估算成一次依赖替换。

1.4 周期的公开 C API 已扩展为完整终端状态等能力；跨平台构建和打包入口也更完整 [2–3]。
这是值得关注的实质进展。与此同时，官方文档仍标记 API 不稳定；1.4 公告称独立库尚未发布
tag，计划再次调整 ABI。核对时仓库 tags 未发现独立 libghostty 版本。
不能把 Ghostty 1.4 应用的发布等同于稳定的 libghostty 1.0 发布。

### 仓库里已有的基础

macOS 的 [`Package.resolved`](../src/frontend/macos/Package.resolved) 锁定 SwiftTerm 1.15.0、
commit `dd2fb8ac5b861e7bf617c872895e338f38165648`。**该确切提交的 README 已声明支持
Sixel、iTerm2 和 Kitty 图片协议，以及可选 Metal 渲染** [6]。
这只是依赖能力的作者说明，本轮没有证明 Muxterm 中已经启用或端到端可用。
Linux GTK VTE 路径也必须独立验证，不能由 macOS 的结果推导。

当前最先要回答的是图片在哪里丢失：应用有没有输出协议、Runtime 有没有交付图片数据与回复、
Surface 是否呈现、恢复时是否还有图片状态。换终端内核不能补回上游已经丢掉的图像数据。

### 如何决定是否迁移

先记录现有引擎的实际失败样例；若现有链路能满足日用图片与阅读需求，就继续使用。
若存在重复的兼容性/性能阻断，或两平台维护分歧持续增加，再做限定范围的 libghostty-vt 验证：

1. 固定上游 revision，先在一个实验 pane 接受现有原始字节；不接管 Runtime、PTY 或 SSH。
   同一 pane 同一时刻只用一个 live Surface；不建“双 VT 对照后向旧 renderer 灌 ANSI”的生产路径。
2. 对照同一组样例：中文/组合字符、IME、选择复制、resize/reflow、全屏 TUI、大量输出与回滚、
   图片和协议回复。测量输入响应、内存和绘制成本，不凭上游宣传预设更快。
3. 第一阶段有实际收益后，再验证 macOS / Linux 的原生交互、打包和下节复用器矩阵。
   写清尚需自行实现的绘制、字体、无障碍和维护成本。
4. 只有实际问题被解决、常用交互无明显回归、两平台有可维护落地路径，才另立迁移任务。
   API 稳定性与升级成本一并评估；实验结果无明显收益则停止，不影响日常功能交付。

若最终迁移，仍遵守 [`SURFACE.md`](SURFACE.md) 与 [`FRONTEND.md`](FRONTEND.md)：
Core 管连接、拓扑和数据；frontend Surface 持有呈现状态；每 pane 一面；隐藏场景继续 feed。
仅替换 Core Index 的解析器不会让 SwiftTerm/VTE 获得新的绘制能力。

## 4. 图片要验证完整链路

| 路径 | 首要验证内容 |
| --- | --- |
| local shell | 实际程序输出、能力探测/回复、显示与删除、滚动和 resize |
| SSH shell | 图像数据确实传到本机；不能把远端文件或共享内存标识当作本机资源 |
| local/SSH tmux | control-mode 下的转义序列、回复路由、pane 归属；首次 attach、detach/reattach 与 gap 恢复 |
| local/SSH Herdr | 观察流/full/diff 是否包含足够图像状态；generation 切换、重连与恢复 |
| macOS / Linux | 各自 Surface 的实际协议与显示结果；资源上限、关闭 pane 后释放 |

Kitty 协议提供远程数据传输与可配合复用器的 Unicode placeholders [8]，
但这不是 Muxterm 的 tmux control-mode 或 Herdr 快照兼容证明。
**首次 live 显示成功与重新连接后恢复成功要分开记录。** 字符/ANSI 快照不自动等于完整图片状态；
无法恢复时要明确限制并保留文件预览入口，不能靠切换 pane 时重新灌屏掩盖。

首批支持范围按实际使用程序选定，不同时追求所有图像协议与动画特性。
图片文件预览独立交付，可以先消除“远端结果看不到”的摩擦。

## 5. 端口与打开：把已有功能连起来

### 已有实现，不重新当作空白项目

- [`ssh_port_discovery.rs`](../src/core/workspace/ssh_port_discovery.rs) 从输出中的
  HTTP/HTTPS loopback URL 发现端口，但只保存 `u16`，未保留 scheme/path/query/fragment；
  当前该解析器匹配 `localhost`、`127.0.0.1`、`0.0.0.0`，未覆盖 IPv6 URL。
- [`muxterm.rs`](../src/core/muxterm.rs) 已有目标机监听端口扫描、异步转发状态、停止/忽略和生命周期处理；
  当前转发入口要求端口已从输出或监听扫描发现，并非任意端口手输功能已经存在。
- [`connection.rs`](../src/core/transport/connection.rs) 分配可用本地端口，经 SSH 转发；
  默认绑定本机回环，可切换 LAN 访问。目标目前固定为远端 `127.0.0.1`。
- macOS [`StatusBarView.swift`](../src/frontend/macos/UI/StatusBarView.swift) 已有打开和复制入口，
  但当前拼的是 `http://host:port`，没有保留原完整链接。
- [`TerminalView.swift`](../src/frontend/macos/Terminal/TerminalView.swift) 的链接入口回调或交给
  系统打开；这不等于已经有按来源机器解析任意远程文件的能力。

### 下一步按用户动作交付

第一项做“点开远端服务的原链接”：例如远端输出
`http://localhost:5173/report/index.html?mode=preview#chart`，点击后在原目标建立/复用转发，
打开 `http://127.0.0.1:<本地端口>/report/index.html?mode=preview#chart`。
来源 Workspace 与打开意图在点击时绑定，切工作区后不能换机器；正在转发与失败可见。

保留原协议、路径、参数和 fragment；端口冲突后使用实际分配端口。HTTPS 不静默改成 HTTP，
验收应覆盖证书/服务域名要求。监听端口未必是网页：只有端口时提供转发/复制，
不能把任意 TCP 服务都宣称可在浏览器中打开。SSH 隧道建立成功也不等于应用已就绪 [11]。
IPv6 需要同时考虑 URL 识别和转发目标，不能只让链接 parser 接受 `[::1]`。

已有端口发现保留；转发由用户触发，不因为扫描到了服务就自动暴露它。
保留本机回环默认值和明确的 LAN 切换，完善停止、失败、关闭工作区与断线后的状态一致性。
手动输入未被发现的端口是候选便利功能，另行定义，不在本轮声称已实现。

第二项做“点开远端文件”：路径连同来源机器、可靠 cwd 一起解析；本地文件交给外部应用，
远程图片做有限预览，普通文件下载副本后打开，远程编辑交给相应外部编辑器。
下载副本不暗示自动回写，失效路径不退回本机同名文件；嵌套 SSH/容器不凭文本猜目标。
复用现有目标连接管理，但不要把图片上传接口当作已完成的通用下载/文件管理能力。

Core 负责目标绑定、传输和转发状态；frontend 负责用户交互与调用系统应用，
保持 [`url_opener.rs`](../src/frontend/url_opener.rs) 所在的副作用边界。
先统一这两类打开行为，不为它们建立新的文件树或浏览器工作台。

## 6. 扩展机制合适，完整插件平台暂缓

文件管理器适合可选工具入口。kitty 的 kittens 展示了通过终端程序扩展能力的路径 [9]；
Yazi 可以作为目标机器上已安装的文件管理 TUI [10]。这是设计参照，不意味着直接兼容 kittens API。

第一步只需要命令面板/快捷键可调用配置好的动作：

- 在指定目标与 cwd 新开 pane/tab 运行文件管理器；保留当前终端，不向正在工作的 agent 盲发命令。
- 将选中的路径交给外部编辑器或打开工具；远程路径保留目标上下文。
- 本机工具与远端工具的执行位置明确，未安装时说明原因；不自动建立另一套连接池。

先落地两个真实用例（文件管理器、外部文件打开），再从重复需求提炼小接口。
若需要返回文件选择结果，再设计简单的进程间结构化输入/输出；不要先承诺 Lua/WASM、
动态库 ABI、常驻插件后台、任意 WebView 面板和市场。

扩展经宿主动作访问所需的 Workspace/Pane/path 上下文，不直接引用 Runtime concrete type，
不抢 FFI 事件泵，不拥有连接池。普通外部进程也不因叫“插件”就自动获得沙箱隔离。
第三方扩展仍会增加接口兼容、错误处理和维护成本，不能把复杂功能全部换名为插件来回避取舍。

默认界面保持普通终端；不安装扩展也能完成 shell、tmux/Herdr 与 Project 的主要工作流。

## 7. 排期边界

下一完整功能仍是编辑框与图片投递；批量复制/阅读、图片呈现与文件打开按独立单元推进。
端口的完整 URL 打开可以作为较小增量，不必等待图片内核选型。
外部工具动作在有真实调用需求时补；libghostty 验证是条件性技术探索，不阻塞上述交付。

完成这些后以日用稳定性、外部用户上手、发布与兼容维护为主。
不需要通过 ACP 客户端、内嵌浏览器或插件市场维持功能增长。

## 8. 官方来源

1. [Ghostty 发布索引](https://ghostty.org/docs/install/release-notes)：核对当前发布版本。
2. [Ghostty 1.4.0](https://ghostty.org/docs/install/release-notes/1-4-0)：图片、剪贴板、HTML formatter、tmux 与 libghostty 进展。
3. [v1.4.0 公开 C 头](https://github.com/ghostty-org/ghostty/blob/v1.4.0/include/ghostty/vt.h)、
   [当前 API 文档](https://libghostty.tip.ghostty.org/index.html)：能力、render state 和不稳定声明。
4. [v1.4.0 内部 embedder 头](https://github.com/ghostty-org/ghostty/blob/v1.4.0/include/ghostty.h)：明确的内部用途与平台边界。
5. [Ghostling 官方示例](https://github.com/ghostty-org/ghostling)：消费者自行绘制；示例不是原生 GUI 成熟度证明。
6. [SwiftTerm 锁定提交 README](https://github.com/migueldeicaza/SwiftTerm/blob/dd2fb8ac5b861e7bf617c872895e338f38165648/README.md)：图像协议与 Metal 能力的上游声明。
7. [Kitty Clipboard Protocol](https://sw.kovidgoyal.net/kitty/clipboard/)：MIME 协商、OSC 5522 与复用器路由。
8. [Kitty Graphics Protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)：远程图像传输、Unicode placeholders 与复用器条件。
9. [kitty kittens](https://sw.kovidgoyal.net/kitty/kittens_intro/)：终端程序式扩展与远程文件功能的参照。
10. [Yazi Quick Start](https://yazi-rs.github.io/docs/quick-start/)：作为现成终端文件管理器的使用方式。
11. [OpenSSH ssh(1)](https://man.openbsd.org/ssh.1)：本地转发与监听地址；
    [ssh_config(5)](https://man.openbsd.org/ssh_config.5#ExitOnForwardFailure)：`ExitOnForwardFailure` 不保证最终服务可连接。

另以 `git ls-remote --tags https://github.com/ghostty-org/ghostty.git` 核对 tags；
发布状态以本次核对时刻为限。上游 API 后续稳定后应重新评估，不能沿用过期结论。
