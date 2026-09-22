//! 构建脚本：把发布 tag 注入二进制，供客户端自更新比较版本。
//!
//! CI 打 tag 时设置 `MUXTERM_BUILD_VERSION=v1.2.3`；本地开发不设置时回退到
//! `Cargo.toml` 的版本号。`rerun-if-env-changed` 保证切换 tag 后 cargo 会重编
//! 依赖该常量的代码，不会复用旧版本号。

fn main() {
    println!("cargo:rerun-if-env-changed=MUXTERM_BUILD_VERSION");
    let version = std::env::var("MUXTERM_BUILD_VERSION")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into()));
    println!("cargo:rustc-env=MUXTERM_BUILD_VERSION={version}");
}
