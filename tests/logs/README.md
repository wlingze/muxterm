# 真实案例日志（本地测试素材）

本目录存放 muxterm 开发时抓取的真实终端/协议日志，用来复现和回归
以下问题：

- `test-2026-0813-1159.log` / `1210`：codex 输入框阶梯、查询应答泄漏
- `test-2026-0813-1308.log`：主题/颜色渲染、status bar 数据
- `test-2026-0813-1320.log`：htop resize 乱屏、输入框光标位置
- `test-2026-0813-1322.log` / `1325`：agent/codex 颜色、渲染时机
- `test-2026-0813-1548.log` / `1630`：codex 长会话（重绘/输入/持续刷新）
- `test-2026-0813-1654.log` / `1702` / `1721` / `1740` / `1745`：
  statusbar 颜色/远程查询、输入换行、agent 重绘漂移
- `a.log` / `b.log` / `c.log` / `yaklang-workspace*.log` / `ubuntu-home.log`：
  pty / SSH / tmux 协议流

这些日志体积大，**不纳入 git 跟踪**（`.gitignore` 的 `*.log` 已覆盖），
只是本地测试素材。需要重新抓取时，用 `muxterm gui --debug --log-file ...`
或直接把终端日志拷进本目录即可。
