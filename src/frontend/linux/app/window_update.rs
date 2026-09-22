//! GTK 侧的更新提醒接线：轮询 Core 状态、渲染 banner、转发用户动作。
//!
//! 所有判断都在 Core；这里只把 `ClientUpdateStatus` 映射到 banner，并把
//! 「一键更新 / 检查 / 关闭提醒」转成 FFI 调用。

use crate::frontend::utils::corebridge::{ClientUpdatePhase, ClientUpdateStatus};

use super::UiState;

/// 每轮 poll 读取 Core 更新状态并刷新 banner。
pub(super) fn poll_update(s: &mut UiState) {
    let Some(status) = s.event_pump.poll_update_status() else {
        return;
    };
    apply_update_status(s, &status);
}

/// 把 Core 状态渲染到 banner（幂等：状态不变时不动 widget）。
pub(super) fn apply_update_status(s: &UiState, status: &ClientUpdateStatus) {
    s.update_banner.apply(status);
}

/// 用户点击 banner 主按钮：按当前阶段触发检查或一键安装。
pub(super) fn handle_update_action(s: &mut UiState) {
    let Some(status) = s.event_pump.poll_update_status() else {
        return;
    };
    let result = match status.phase {
        ClientUpdatePhase::Available => s.event_pump.client().update_install(),
        ClientUpdatePhase::Failed => s.event_pump.client().update_check(),
        _ => Ok(status),
    };
    match result {
        Ok(updated) => apply_update_status(s, &updated),
        Err(error) => tracing::warn!(
            target = "muxterm::update",
            %error,
            "更新动作未能提交给 Core"
        ),
    }
}

/// 用户关掉提醒：记住当前版本，不再重复弹出。
pub(super) fn dismiss_update(s: &mut UiState) {
    let version = s.update_banner.current_version();
    s.update_banner.dismiss(version);
}

/// 「检查更新」入口（命令面板 / 设置页共用）。
pub(super) fn check_for_updates(s: &mut UiState) {
    match s.event_pump.client().update_check() {
        Ok(status) => apply_update_status(s, &status),
        Err(error) => tracing::warn!(
            target = "muxterm::update",
            %error,
            "检查更新未能提交给 Core"
        ),
    }
}
