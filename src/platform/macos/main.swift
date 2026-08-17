import AppKit
import MuxtermAppLib

// Muxterm macOS 原生客户端入口。
let app = NSApplication.shared
// 尽早声明为常规 GUI，避免启动瞬间被当成 background-only
app.setActivationPolicy(.regular)
let delegate = AppDelegate()
app.delegate = delegate
app.run()
