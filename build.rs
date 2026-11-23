//! 构建脚本：把发布 tag 注入二进制，供客户端自更新比较版本。
//!
//! CI 打 tag 时设置 `MUXTERM_BUILD_VERSION=v1.2.3`；本地开发不设置时回退到
//! `Cargo.toml` 的版本号。`rerun-if-env-changed` 保证切换 tag 后 cargo 会重编
//! 依赖该常量的代码，不会复用旧版本号。

fn main() {
    embed_windows_icon();
    println!("cargo:rerun-if-env-changed=MUXTERM_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=MUXTERM_GIT_COMMIT");
    println!("cargo:rerun-if-changed=.git/HEAD");
    let version = std::env::var("MUXTERM_BUILD_VERSION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into()));
    println!("cargo:rustc-env=MUXTERM_BUILD_VERSION={version}");
    println!("cargo:rustc-env=MUXTERM_GIT_COMMIT={}", git_commit());
}

/// Windows：把 `assets/icons/muxterm.rc` 编译进 exe。
///
/// gpui 创建窗口时 `LoadImageW(module, 1, IMAGE_ICON)` 读的正是资源 ID 1；
/// 没有这段，窗口/任务栏/Explorer 全是通用图标。非 Windows 构建是空操作。
fn embed_windows_icon() {
    println!("cargo:rerun-if-changed=assets/icons/muxterm.rc");
    println!("cargo:rerun-if-changed=assets/icons/muxterm.ico");
    #[cfg(windows)]
    {
        if let Err(error) = embed_resource::compile("assets/icons/muxterm.rc", embed_resource::NONE)
            .manifest_optional()
        {
            // 图标是装饰性的：编译失败不要挡住发布，但要让日志可见。
            println!("cargo:warning=嵌入 Windows 图标失败: {error}");
        }
    }
}

fn git_commit() -> String {
    if let Ok(value) = std::env::var("MUXTERM_GIT_COMMIT") {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".into())
}
