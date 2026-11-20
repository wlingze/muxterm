# vendor/xattr — 上游 xattr 0.2.3 + Linux 编译修复

为什么放进仓库：`gpui 0.2.2` 的依赖链是
`gpui → gpui_http_client → zed-async-tar → xattr 0.2.3`。上游 0.2.3 在
`src/sys/mod.rs` 的 `platforms!` 宏里无条件生成

```rust
pub const ENOATTR: libc::c_int = libc::ENOATTR;
```

`libc` 在 Linux / Android 上没有 `ENOATTR`（只有 `ENODATA`），于是 Linux
上整个依赖树编不过，GPUI 前端无法在 Linux 构建。

本目录是 crates.io 上 `xattr 0.2.3` 的**原样拷贝**，只改了
`src/sys/mod.rs` 里的这一行：Linux / Android 用 `libc::ENODATA`，其它平台
保持 `libc::ENOATTR`。没有改任何函数行为。

- 上游仓库：<https://github.com/Stebalien/xattr>
- 上游修复方式（1.x 起）：Linux 走 `rustix::io::Errno::NODATA`。
- 通过根目录 `Cargo.toml` 的 `[patch.crates-io]` 接入。
- 许可：MIT / Apache-2.0（见本目录 `LICENSE-*`）。

升级 gpui 或 xattr 后应删除本目录与对应的 `[patch.crates-io]` 行。
