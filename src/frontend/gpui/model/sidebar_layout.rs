//! 侧栏几何：可拖动宽度 + 上下半区的分段高度。
//!
//! 用户报障（截图 1）：侧栏宽度写死 240pt 不能拖；四段挤在同一个滚动区里，
//! Workspaces 与 Commands/Agents 抢同一条滚动条，看着「全堆在一起」。
//!
//! 目标布局：
//! - 上半区：Workspaces（可滚动，占满上半区剩下的高度）。
//! - 下半区：Agents 比 Commands 大（2:1），Hidden Commands 固定在整栏最底部、
//!   默认收起、不参与比例分配。
//!
//! 对照 macOS `Chrome/SidebarSectionSplitLayout.swift`：折叠段固定
//! `collapsedHeight = 26`，展开段至少 `expandedMinHeight = 80`；高度不够时
//! 返回 `None`（调用方不能硬排，否则 `{0,-25}` 那种乱序会让侧栏看着像消失）。

/// 侧栏默认宽度：与 macOS `mainSplitController.setPosition(240)` 一致。
pub const SIDEBAR_WIDTH: f32 = 240.0;
/// 拖动下限：Swift `sidebarItem.minimumThickness = 200`。
pub const SIDEBAR_MIN_WIDTH: f32 = 200.0;
/// 拖动上限：Swift `sidebarItem.maximumThickness = 480`。
pub const SIDEBAR_MAX_WIDTH: f32 = 480.0;
/// 分隔条命中宽度（拖动热区）。视觉上只有 1pt 线，热区宽一点才好抓。
pub const SIDEBAR_DIVIDER_HIT_WIDTH: f32 = 5.0;

/// 折叠段高度：Swift `collapsedHeight`。
pub const SECTION_COLLAPSED_HEIGHT: f32 = 26.0;
/// 展开段最小高度：Swift `expandedMinHeight`。
pub const SECTION_EXPANDED_MIN_HEIGHT: f32 = 80.0;

/// 三段的高度权重：Workspaces : Agents : Commands。
///
/// 「Agents 拿更多、Commands 拿更少」是用户明确的视觉要求：Agent 行带副标题
/// 与状态，本来就比命令行信息密度高。比例取 2:1 同时让 Workspaces 拿到最多。
pub const SECTION_WEIGHTS: (f32, f32, f32) = (3.0, 2.0, 1.0);

/// 把一次拖动后的宽度夹进 `[min, max]`，并保证正文至少还有 `min_main` 宽。
///
/// 只在拖动时调用：窗口被拖窄时不主动改用户选的宽度（Swift 的
/// `NSSplitViewController` 也不会把 autosave 的值改小）。
pub fn clamp_sidebar_width(width: f32, window_width: f32, min_main: f32) -> f32 {
    let ceiling = (window_width - min_main).max(SIDEBAR_MIN_WIDTH);
    width.clamp(SIDEBAR_MIN_WIDTH, ceiling.min(SIDEBAR_MAX_WIDTH))
}

/// 三段的垂直高度：上半区（Workspaces）、Agents、Commands。
///
/// `hidden_height` 是固定在底部的 Hidden Commands 段占的高度（收起时就是段头
/// 高度）。返回 `None` 表示高度不够，调用方必须跳过这一帧的排布。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarSectionHeights {
    pub workspaces: f32,
    pub agents: f32,
    pub commands: f32,
}

impl SidebarSectionHeights {
    /// `available` 是整栏可用高度（已扣掉 padding）；`hidden_height` 是底部固定段。
    ///
    /// 规则：
    /// 1. 按 [`SECTION_WEIGHTS`] 分高度；
    /// 2. Commands 低于展开下限时从 Workspaces 补（它可滚动，最能被挤）；
    /// 3. Workspaces 被挤到下限以下时，把下半区按 2:1 压回去；
    /// 4. 连三段下限都放不下就返回 `None`。
    pub fn compute(available: f32, hidden_height: f32) -> Option<Self> {
        let body = available - hidden_height;
        let (workspaces_weight, agents_weight, commands_weight) = SECTION_WEIGHTS;
        let total_weight = workspaces_weight + agents_weight + commands_weight;
        let min_total = SECTION_EXPANDED_MIN_HEIGHT * 3.0;
        if body + 0.5 < min_total {
            return None;
        }
        let mut workspaces = body * workspaces_weight / total_weight;
        let mut agents = body * agents_weight / total_weight;
        let mut commands = body * commands_weight / total_weight;
        if commands < SECTION_EXPANDED_MIN_HEIGHT {
            let shortfall = SECTION_EXPANDED_MIN_HEIGHT - commands;
            commands += shortfall;
            workspaces -= shortfall;
        }
        if workspaces < SECTION_EXPANDED_MIN_HEIGHT {
            workspaces = SECTION_EXPANDED_MIN_HEIGHT;
            let bottom = body - workspaces;
            agents = bottom * 2.0 / 3.0;
            commands = bottom / 3.0;
        }
        Some(Self {
            workspaces,
            agents,
            commands,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用户报障：侧栏宽度必须能拖，且不能把正文挤没。
    #[test]
    fn dragging_the_divider_stays_within_the_swift_limits() {
        // 正常拖动。
        assert_eq!(clamp_sidebar_width(300.0, 1_200.0, 320.0), 300.0);
        // 拖太左：夹到 200。
        assert_eq!(clamp_sidebar_width(40.0, 1_200.0, 320.0), SIDEBAR_MIN_WIDTH);
        // 拖太右：夹到 480（Swift maximumThickness）。
        assert_eq!(
            clamp_sidebar_width(900.0, 1_200.0, 320.0),
            SIDEBAR_MAX_WIDTH
        );
        // 窗口很窄：正文优先，右界跟着缩小。
        assert_eq!(clamp_sidebar_width(900.0, 520.0, 320.0), SIDEBAR_MIN_WIDTH);
    }

    /// Agents 必须比 Commands 大，Hidden Commands 不参与比例分配。
    #[test]
    fn agents_get_twice_the_commands_height() {
        let heights = SidebarSectionHeights::compute(800.0, 26.0).expect("高度足够");
        assert!(
            (heights.agents - heights.commands * 2.0).abs() < 0.5,
            "Agents 必须是 Commands 的两倍：{heights:?}"
        );
        assert!(
            heights.workspaces > heights.agents,
            "上半区要占主要高度：{heights:?}"
        );
        let total = heights.workspaces + heights.agents + heights.commands + 26.0;
        assert!((total - 800.0).abs() < 0.5, "不能凭空多出或丢掉高度");
    }

    /// 高度不够时返回 `None`：宁可不排，也不要排出负高度。
    #[test]
    fn too_little_height_refuses_to_lay_out() {
        assert!(SidebarSectionHeights::compute(120.0, 26.0).is_none());
        assert!(SidebarSectionHeights::compute(26.0, 26.0).is_none());
        assert!(SidebarSectionHeights::compute(0.0, 26.0).is_none());
        // 刚好够：每段都到展开下限。
        let tight = SidebarSectionHeights::compute(26.0 + 240.0, 26.0);
        assert!(tight.is_some(), "三段恰好都到下限时应当能排：{tight:?}");
    }

    /// 底部固定段越高，上半区越矮，且总高度守恒。
    #[test]
    fn the_pinned_hidden_section_takes_its_height_off_the_top() {
        let with_hidden = SidebarSectionHeights::compute(600.0, 26.0 + 120.0).unwrap();
        let without_hidden = SidebarSectionHeights::compute(600.0, 26.0).unwrap();
        assert!(with_hidden.workspaces < without_hidden.workspaces);
    }
}
