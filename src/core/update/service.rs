//! 更新状态机：检查 → 有可用更新 → 一键安装 → 待重启。
//!
//! 网络与磁盘动作都在后台线程执行；FFI 线程只做状态跃迁与事件入队，
//! 因此「提醒更新」不会阻塞界面。传输层抽象成 [`UpdateTransport`]，
//! 单元测试注入替身，不需要真实网络。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;

use super::install::{self, InstallOutcome, UpdateError};
use super::manifest::{Asset, Manifest};
use super::platform::current_asset_key;
use super::REPOSITORY_URL;

/// 检查/下载使用的网络出口；生产实现走 HTTP，测试注入替身。
pub trait UpdateTransport: Send + Sync {
    fn fetch_manifest(&self, url: &str) -> Result<Manifest, UpdateError>;
    /// 下载资产。`url` 是已按清单地址解析过的绝对地址。
    fn download(&self, asset: &Asset, url: &str, destination: &Path) -> Result<(), UpdateError>;
}

/// 基于 ureq 的生产实现。
pub struct HttpTransport;

impl UpdateTransport for HttpTransport {
    fn fetch_manifest(&self, url: &str) -> Result<Manifest, UpdateError> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(super::NETWORK_TIMEOUT))
            .build()
            .into();
        let text = agent
            .get(url)
            .call()
            .map_err(|error| UpdateError::Network(error.to_string()))?
            .body_mut()
            .read_to_string()
            .map_err(|error| UpdateError::Network(error.to_string()))?;
        Manifest::parse(&text).map_err(|error| UpdateError::Network(error.to_string()))
    }

    fn download(&self, asset: &Asset, url: &str, destination: &Path) -> Result<(), UpdateError> {
        install::download_asset_from(asset, url, destination)
    }
}

/// 更新流程对外可见的阶段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdatePhase {
    /// 还没检查过。
    Idle,
    /// 正在检查发布信息。
    Checking,
    /// 已是最新版本。
    UpToDate,
    /// 发现新版本，等待用户点击一键更新。
    Available { version: String },
    /// 正在下载/安装。
    Installing { version: String },
    /// 安装完成，等待重启生效。
    Installed { version: String },
    /// 出错。
    Failed { message: String },
}

impl UpdatePhase {
    /// FFI 状态 JSON 里的稳定阶段名。
    pub fn as_str(&self) -> &'static str {
        match self {
            UpdatePhase::Idle => "idle",
            UpdatePhase::Checking => "checking",
            UpdatePhase::UpToDate => "up_to_date",
            UpdatePhase::Available { .. } => "available",
            UpdatePhase::Installing { .. } => "installing",
            UpdatePhase::Installed { .. } => "installed",
            UpdatePhase::Failed { .. } => "failed",
        }
    }

    /// 是否已经安装完成、需要重启才能生效。
    pub fn restart_required(&self) -> bool {
        matches!(self, UpdatePhase::Installed { .. })
    }
}

/// 后台任务结果。
enum Outcome {
    Checked {
        result: Result<Manifest, UpdateError>,
        /// 是否由用户主动触发；自动检查失败不打扰界面。
        manual: bool,
    },
    Installed(Result<InstallOutcome, UpdateError>),
}

/// 更新服务：一个产品会话一份。
pub struct UpdateService {
    transport: Arc<dyn UpdateTransport>,
    phase: UpdatePhase,
    /// 已发现的新版本清单；安装时直接用，不重新查询。
    manifest: Option<Manifest>,
    /// 当前二进制的版本号。
    current_version: String,
    /// 清单地址（可被配置覆盖）。
    manifest_url: String,
    /// 是否启动后自动检查一次（默认开启，只提醒不自动安装）。
    auto_check: bool,
    /// 本次进程是否已触发过自动检查，避免每个 poll 都发请求。
    auto_check_launched: bool,
    /// 上一次自动检查的时间，用于节流。
    last_auto_check: Option<std::time::Instant>,
    pending: Option<Receiver<Outcome>>,
    /// 当前检查是否由用户主动触发（决定失败是否可见）。
    checking_is_manual: bool,
    events: VecDeque<serde_json::Value>,
}

/// 自动检查的最小间隔：同一会话内不反复打扰网络。
const AUTO_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 60 * 60);

impl UpdateService {
    pub fn new(
        transport: Arc<dyn UpdateTransport>,
        auto_check: bool,
        manifest_url: String,
    ) -> Self {
        Self {
            transport,
            phase: UpdatePhase::Idle,
            manifest: None,
            current_version: super::BUILD_VERSION.to_string(),
            manifest_url: if manifest_url.trim().is_empty() {
                super::DEFAULT_MANIFEST_URL.to_string()
            } else {
                manifest_url
            },
            auto_check,
            auto_check_launched: false,
            last_auto_check: None,
            pending: None,
            checking_is_manual: false,
            events: VecDeque::new(),
        }
    }

    /// 生产构造：HTTP 传输 + 构建期版本号。
    pub fn http(auto_check: bool, manifest_url: String) -> Self {
        Self::new(Arc::new(HttpTransport), auto_check, manifest_url)
    }

    /// 覆盖当前版本号（测试与本地开发用）。
    pub fn with_current_version(mut self, version: impl Into<String>) -> Self {
        self.current_version = version.into();
        self
    }

    /// 本地测试/开发用：用环境变量覆盖当前版本号与清单地址。
    ///
    /// 只影响显式设置了变量的进程，方便在没有正式 release 时用本地 HTTP
    /// 服务验证「提醒 + 一键更新」整条链路。
    pub fn apply_env_overrides(mut self) -> Self {
        if let Ok(version) = std::env::var(VERSION_ENV) {
            let version = version.trim();
            if !version.is_empty() {
                self.current_version = version.to_string();
            }
        }
        if let Ok(url) = std::env::var(MANIFEST_URL_ENV) {
            let url = url.trim();
            if !url.is_empty() {
                self.manifest_url = url.to_string();
            }
        }
        self
    }

    pub fn phase(&self) -> &UpdatePhase {
        &self.phase
    }

    pub fn current_version(&self) -> &str {
        &self.current_version
    }

    pub fn manifest_url(&self) -> &str {
        &self.manifest_url
    }

    /// 是否正在跑后台任务。
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// 自动检查：启动后触发一次；同一会话内按 [`AUTO_CHECK_INTERVAL`] 节流。
    pub fn maybe_auto_check(&mut self) {
        if !self.auto_check || self.pending.is_some() {
            return;
        }
        if matches!(
            self.phase,
            UpdatePhase::Available { .. }
                | UpdatePhase::Installing { .. }
                | UpdatePhase::Installed { .. }
        ) {
            return;
        }
        let due = self
            .last_auto_check
            .is_none_or(|last| last.elapsed() >= AUTO_CHECK_INTERVAL && !self.auto_check_launched);
        if self.auto_check_launched && !due {
            return;
        }
        if self.start_check_with_origin(false) {
            self.auto_check_launched = true;
            self.last_auto_check = Some(std::time::Instant::now());
        }
    }

    /// 手动检查。已有任务在跑时忽略，返回 false。
    pub fn start_check(&mut self) -> bool {
        self.start_check_with_origin(true)
    }

    fn start_check_with_origin(&mut self, manual: bool) -> bool {
        if self.pending.is_some() {
            return false;
        }
        let transport = Arc::clone(&self.transport);
        let url = self.manifest_url.clone();
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("muxterm-update-check".into())
            .spawn(move || {
                let _ = sender.send(Outcome::Checked {
                    result: transport.fetch_manifest(&url),
                    manual,
                });
            });
        if spawned.is_err() {
            self.set_phase(UpdatePhase::Failed {
                message: "无法启动更新检查线程".into(),
            });
            return false;
        }
        self.pending = Some(receiver);
        self.checking_is_manual = manual;
        self.set_phase(UpdatePhase::Checking);
        true
    }

    /// 一键更新：下载 + 校验 + 安装。需要先有可用更新。
    pub fn start_install(&mut self) -> bool {
        if self.pending.is_some() {
            return false;
        }
        let Some(manifest) = self.manifest.clone() else {
            self.set_phase(UpdatePhase::Failed {
                message: "还没有可安装的新版本，请先检查更新".into(),
            });
            return false;
        };
        let Some(key) = current_asset_key() else {
            self.set_phase(UpdatePhase::Failed {
                message: "当前平台没有可用的更新包".into(),
            });
            return false;
        };
        let asset = match manifest.asset(key) {
            Ok(asset) => asset.clone(),
            Err(error) => {
                self.set_phase(UpdatePhase::Failed {
                    message: error.to_string(),
                });
                return false;
            }
        };
        let version = manifest.version.clone();
        let transport = Arc::clone(&self.transport);
        let staging = super::staging_dir();
        let install_version = version.clone();
        let manifest_url = self.manifest_url.clone();
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("muxterm-update-install".into())
            .spawn(move || {
                let result = install::download_and_install(
                    transport.as_ref(),
                    &asset,
                    &install_version,
                    &manifest_url,
                    &staging,
                );
                let _ = sender.send(Outcome::Installed(result));
            });
        if spawned.is_err() {
            self.set_phase(UpdatePhase::Failed {
                message: "无法启动更新安装线程".into(),
            });
            return false;
        }
        self.pending = Some(receiver);
        self.set_phase(UpdatePhase::Installing { version });
        true
    }

    /// 主线程 poll：采纳后台结果并排出状态事件。
    pub fn poll(&mut self) {
        let Some(receiver) = self.pending.take() else {
            return;
        };
        match receiver.try_recv() {
            Ok(Outcome::Checked {
                result: Ok(manifest),
                ..
            }) => {
                if manifest.is_newer_than(&self.current_version) {
                    self.set_phase(UpdatePhase::Available {
                        version: manifest.version.clone(),
                    });
                    self.manifest = Some(manifest);
                } else {
                    self.manifest = None;
                    self.set_phase(UpdatePhase::UpToDate);
                }
            }
            Ok(Outcome::Checked {
                result: Err(error),
                manual,
            }) => {
                if manual || self.checking_is_manual {
                    self.set_phase(UpdatePhase::Failed {
                        message: error.to_string(),
                    });
                } else {
                    // 自动检查失败（离线、限流、还没有 release）不弹提醒，
                    // 回到 idle 等下一次机会，避免开机就报错。
                    tracing::debug!(
                        target = "muxterm::update",
                        %error,
                        "自动检查更新失败，已静默忽略"
                    );
                    self.set_phase(UpdatePhase::Idle);
                }
            }
            Ok(Outcome::Installed(Ok(outcome))) => {
                self.set_phase(UpdatePhase::Installed {
                    version: outcome.version.clone(),
                });
            }
            Ok(Outcome::Installed(Err(error))) => {
                let version = self
                    .manifest
                    .as_ref()
                    .map(|manifest| manifest.version.clone())
                    .unwrap_or_default();
                self.set_phase(UpdatePhase::Failed {
                    message: if version.is_empty() {
                        error.to_string()
                    } else {
                        format!("{version} 安装失败: {error}")
                    },
                });
            }
            Err(TryRecvError::Empty) => {
                // 还没完成，放回去等下一次 poll。
                self.pending = Some(receiver);
            }
            Err(TryRecvError::Disconnected) => {
                self.set_phase(UpdatePhase::Failed {
                    message: "更新任务意外结束".into(),
                });
            }
        }
    }

    /// 当前状态快照（供前端首帧渲染与事件载荷）。
    pub fn status_json(&self) -> serde_json::Value {
        let mut value = serde_json::json!({
            "phase": self.phase.as_str(),
            "current_version": self.current_version,
            "restart_required": self.phase.restart_required(),
            "release_url": format!("{REPOSITORY_URL}/releases"),
        });
        match &self.phase {
            UpdatePhase::Available { version } | UpdatePhase::Installing { version } => {
                value["version"] = serde_json::json!(version);
            }
            UpdatePhase::Installed { version } => {
                value["version"] = serde_json::json!(version);
            }
            UpdatePhase::Failed { message } => {
                value["message"] = serde_json::json!(message);
            }
            _ => {}
        }
        if let Some(manifest) = &self.manifest {
            let tag = manifest
                .tag
                .clone()
                .unwrap_or_else(|| manifest.version.clone());
            value["release_url"] =
                serde_json::json!(format!("{REPOSITORY_URL}/releases/tag/{tag}"));
            if let Some(key) = current_asset_key() {
                if let Some(asset) = manifest.assets.get(key) {
                    // 清单里可以是相对路径，对外统一给可直接下载的绝对地址。
                    value["download_url"] = serde_json::json!(super::manifest::resolve_asset_url(
                        &self.manifest_url,
                        &asset.url
                    ));
                    value["asset_name"] = serde_json::json!(asset.name);
                }
            }
        }
        value
    }

    /// 取走待发送的状态变更事件。返回 `{"events":[...]}`。
    pub fn take_events_json(&mut self) -> serde_json::Value {
        let events: Vec<_> = self.events.drain(..).collect();
        serde_json::json!({ "events": events })
    }

    fn set_phase(&mut self, phase: UpdatePhase) {
        if self.phase == phase {
            return;
        }
        self.phase = phase;
        self.events.push_back(serde_json::json!({
            "type": "status",
            "status": self.status_json(),
        }));
    }
}

/// 覆盖当前版本号（本地把 alpha 包当成「旧版本」来验证更新）。
pub const VERSION_ENV: &str = "MUXTERM_UPDATE_VERSION";
/// 覆盖清单地址（本地指向 `python3 -m http.server` 输出的 latest.json）。
pub const MANIFEST_URL_ENV: &str = "MUXTERM_UPDATE_MANIFEST_URL";

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 测试替身：返回固定清单，不做真实网络。
    struct FakeTransport {
        manifest: Result<Manifest, UpdateError>,
        downloads: Mutex<Vec<String>>,
        bytes: Vec<u8>,
        corrupt_checksum: bool,
    }

    impl UpdateTransport for FakeTransport {
        fn fetch_manifest(&self, _url: &str) -> Result<Manifest, UpdateError> {
            match &self.manifest {
                Ok(manifest) => Ok(manifest.clone()),
                Err(error) => Err(UpdateError::Network(error.to_string())),
            }
        }

        fn download(
            &self,
            asset: &Asset,
            _url: &str,
            destination: &Path,
        ) -> Result<(), UpdateError> {
            self.downloads
                .lock()
                .unwrap()
                .push(destination.display().to_string());
            let mut asset = asset.clone();
            if self.corrupt_checksum {
                // 让清单摘要与真实内容不匹配，验证安装前的二次校验会拦住它。
                asset.sha256 = "deadbeef".into();
            }
            if let Some(parent) = destination.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(destination, self.bytes.clone())?;
            Ok(())
        }
    }

    fn manifest_json(version: &str) -> String {
        format!(
            r#"{{"version":"{version}","tag":"{version}","assets":{{"macos-arm64":{{"name":"muxterm-macos-arm64.dmg","url":"https://example.invalid/a.dmg","sha256":"aa","size":1}},"linux-gui-x86_64":{{"name":"muxterm-gtk-linux-x86_64.tar.gz","url":"https://example.invalid/a.tar.gz","sha256":"bb","size":1}}}}}}"#
        )
    }

    fn service(version: &str, current: &str) -> UpdateService {
        let transport = FakeTransport {
            manifest: Ok(Manifest::parse(&manifest_json(version)).unwrap()),
            downloads: Mutex::new(Vec::new()),
            bytes: Vec::new(),
            corrupt_checksum: false,
        };
        UpdateService::new(Arc::new(transport), false, String::new()).with_current_version(current)
    }

    fn wait_for<F: Fn(&UpdateService) -> bool>(service: &mut UpdateService, predicate: F) -> bool {
        for _ in 0..300 {
            service.poll();
            if predicate(service) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn newer_release_moves_to_available_and_emits_event() {
        let mut service = service("v2.0.0", "v1.0.0");
        assert!(service.start_check());
        assert!(wait_for(&mut service, |service| matches!(
            service.phase(),
            UpdatePhase::Available { .. }
        )));
        assert_eq!(service.status_json()["phase"], "available");
        assert_eq!(service.status_json()["version"], "v2.0.0");
        assert_eq!(service.status_json()["restart_required"], false);

        let events = service.take_events_json();
        assert!(
            events["events"].as_array().unwrap().len() >= 2,
            "检查中与已发现各应产生一次状态事件: {events}"
        );
        assert!(service.take_events_json()["events"]
            .as_array()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn same_or_older_release_reports_up_to_date() {
        let mut service = service("v1.0.0", "v1.0.0");
        service.start_check();
        assert!(wait_for(&mut service, |service| matches!(
            service.phase(),
            UpdatePhase::UpToDate
        )));
        assert_eq!(service.status_json()["phase"], "up_to_date");
        // 已是最新时没有待安装清单，一键更新被拒绝（并记为失败提示）。
        assert!(!service.start_install(), "已是最新时不应允许安装");
    }

    #[test]
    fn check_failure_is_reported_without_crashing() {
        let transport = FakeTransport {
            manifest: Err(UpdateError::Network("boom".into())),
            downloads: Mutex::new(Vec::new()),
            bytes: Vec::new(),
            corrupt_checksum: false,
        };
        let mut service = UpdateService::new(Arc::new(transport), false, String::new())
            .with_current_version("v1.0.0");
        service.start_check();
        assert!(wait_for(&mut service, |service| matches!(
            service.phase(),
            UpdatePhase::Failed { .. }
        )));
        assert!(service.status_json()["message"]
            .as_str()
            .unwrap()
            .contains("boom"));
    }

    #[test]
    fn auto_check_fires_once_per_session() {
        let transport = FakeTransport {
            manifest: Ok(Manifest::parse(&manifest_json("v2.0.0")).unwrap()),
            downloads: Mutex::new(Vec::new()),
            bytes: Vec::new(),
            corrupt_checksum: false,
        };
        let mut service = UpdateService::new(Arc::new(transport), true, String::new())
            .with_current_version("v1.0.0");
        service.maybe_auto_check();
        assert!(wait_for(&mut service, |service| matches!(
            service.phase(),
            UpdatePhase::Available { .. }
        )));
        // 已进入 available 后不再重复检查。
        service.maybe_auto_check();
        assert!(matches!(service.phase(), UpdatePhase::Available { .. }));
    }

    #[test]
    fn auto_check_is_disabled_by_config() {
        let mut service = service("v2.0.0", "v1.0.0");
        service.maybe_auto_check();
        assert_eq!(service.status_json()["phase"], "idle");
        assert!(!service.is_busy());
    }

    #[test]
    fn installing_an_unsupported_platform_asset_fails_cleanly() {
        let mut service = service("v2.0.0", "v1.0.0");
        assert!(!service.start_install(), "未检查前不应允许安装");
        assert!(matches!(service.phase(), UpdatePhase::Failed { .. }));
    }

    #[test]
    fn busy_service_refuses_a_second_check() {
        let mut service = service("v2.0.0", "v1.0.0");
        assert!(service.start_check());
        assert!(!service.start_check(), "同一时间只允许一个检查任务");
        let _ = wait_for(&mut service, |service| !service.is_busy());
    }

    #[test]
    fn status_reports_an_absolute_download_url_for_relative_manifest_entries() {
        // 只有两个正式发布平台有资产；其余平台（如 Linux ARM 开发容器）
        // 本就不该给出下载链接，直接跳过。
        let Some(key) = crate::update::platform::current_asset_key() else {
            return;
        };
        let (name, url) = match key {
            crate::update::platform::MACOS_ARM64 => {
                ("muxterm-macos-arm64.dmg", "muxterm-macos-arm64.dmg")
            }
            crate::update::platform::LINUX_GUI_X86_64 => (
                "muxterm-gtk-linux-x86_64.tar.gz",
                "muxterm-gtk-linux-x86_64.tar.gz",
            ),
            other => panic!("测试只覆盖两个正式发布平台，实际 key={other}"),
        };
        let manifest_json = format!(
            r#"{{"version":"v9.9.9","assets":{{"{key}":{{"name":"{name}","url":"{url}","sha256":"aa"}}}}}}"#
        );

        struct RelativeTransport(String);
        impl UpdateTransport for RelativeTransport {
            fn fetch_manifest(&self, _url: &str) -> Result<Manifest, UpdateError> {
                Ok(Manifest::parse(&self.0).unwrap())
            }
            fn download(
                &self,
                _asset: &Asset,
                _url: &str,
                _destination: &Path,
            ) -> Result<(), UpdateError> {
                Ok(())
            }
        }

        let mut service = UpdateService::new(
            Arc::new(RelativeTransport(manifest_json)),
            false,
            "https://example.invalid/releases/download/v9.9.9/latest.json".into(),
        )
        .with_current_version("v1.0.0");
        service.start_check();
        assert!(wait_for(&mut service, |service| matches!(
            service.phase(),
            UpdatePhase::Available { .. }
        )));
        let status = service.status_json();
        assert_eq!(status["asset_name"].as_str(), Some(name));
        let url = status["download_url"].as_str().unwrap();
        assert!(
            url.starts_with("https://example.invalid/releases/download/v9.9.9/"),
            "相对路径必须解析成绝对地址: {url}"
        );
        assert!(
            url.ends_with(name),
            "解析出的地址必须指向当前平台的资产，期望以 {name} 结尾: {url}"
        );
    }
}
