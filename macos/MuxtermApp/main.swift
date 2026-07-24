import AppKit

// Muxterm macOS 原生客户端入口。
let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.run()
