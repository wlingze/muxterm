//! Quick Pick 的纯模型与过滤逻辑。
//!
//! 这里不依赖 GTK；Overlay 只负责把模型渲染成列表并转发用户手势。

use crate::frontend::i18n::{self, Key as TextKey};

pub const ENTRY_HEIGHT: i32 = 36;

/// 一条可选项。
#[derive(Debug, Clone)]
pub struct QuickPickItem {
    pub id: String,
    pub label: String,
    pub detail: Option<String>,
}

/// 根据父窗口高度计算面板/列表高度（纯函数，保证列表不溢出）。
/// 返回 `(panel_h, list_h)`。
pub fn panel_list_heights(parent_h: i32) -> (i32, i32) {
    let panel_h = (parent_h / 2).clamp(200, 420);
    let list_h = (panel_h - ENTRY_HEIGHT - 8).max(100);
    (panel_h, list_h)
}

/// 按 query 过滤候选项（label / detail 模糊匹配）。
pub fn filter_items(items: &[QuickPickItem], query: &str) -> Vec<QuickPickItem> {
    items
        .iter()
        .filter(|it| {
            fuzzy_match(query, &it.label)
                || it.detail.as_ref().is_some_and(|d| fuzzy_match(query, d))
        })
        .cloned()
        .collect()
}

/// 带自由输入的 Quick Pick：输入框非空时，始终把当前文本作为首选项。
///
/// 用于 SSH 目标等「可从列表选、也可直接敲」的场景。选中自由输入项时
/// `id == FREEFORM_ID`。
pub const FREEFORM_ID: &str = "__typed__";

/// 自由输入过滤（纯函数）：query 非空时首项为 typed target。
pub fn freeform_filter(presets: &[QuickPickItem], query: &str) -> Vec<QuickPickItem> {
    let mut next = Vec::new();
    let qtrim = query.trim();
    if !qtrim.is_empty() {
        next.push(QuickPickItem {
            id: FREEFORM_ID.into(),
            label: qtrim.to_string(),
            detail: Some(i18n::tr(TextKey::FreeformUseTypedTarget)),
        });
    }
    for it in presets {
        if qtrim.is_empty()
            || fuzzy_match(qtrim, &it.label)
            || it.detail.as_ref().is_some_and(|d| fuzzy_match(qtrim, d))
        {
            next.push(it.clone());
        }
    }
    next
}

/// 模糊匹配：查询的每个字符按序出现在目标中（大小写不敏感）。
pub fn fuzzy_match(query: &str, target: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    let t = target.to_lowercase();
    if t.contains(&q) {
        return true;
    }
    let mut ti = t.chars().peekable();
    for qc in q.chars() {
        loop {
            match ti.next() {
                Some(tc) if tc == qc => break,
                Some(_) => continue,
                None => return false,
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_empty_matches_all() {
        assert!(fuzzy_match("", "tmux: attach"));
    }

    #[test]
    fn fuzzy_substring() {
        assert!(fuzzy_match("tmux", "tmux: attach to session"));
        assert!(fuzzy_match("tab", "new tab"));
        assert!(!fuzzy_match("zzz", "new tab"));
    }

    #[test]
    fn fuzzy_subsequence() {
        assert!(fuzzy_match("ntb", "new tab"));
        assert!(fuzzy_match("tcns", "tmux: create new session"));
    }

    #[test]
    fn fuzzy_match_is_case_insensitive() {
        assert!(fuzzy_match("TMUX", "tmux: attach"));
        assert!(fuzzy_match("NeW tAb", "new tab"));
        assert!(fuzzy_match("ntb", "NEW TAB"));
    }

    #[test]
    fn fuzzy_match_supports_chinese() {
        assert!(fuzzy_match("命令", "打开命令面板"));
        assert!(fuzzy_match("面板", "打开命令面板"));
        assert!(!fuzzy_match("窗口", "打开命令面板"));
    }

    #[test]
    fn fuzzy_match_rejects_missing_characters() {
        assert!(!fuzzy_match("zzz", "new tab"));
        assert!(!fuzzy_match("abcdef", "ab"));
    }

    #[test]
    fn filter_preserves_order() {
        let items = vec![
            QuickPickItem {
                id: "a".into(),
                label: "new tab".into(),
                detail: None,
            },
            QuickPickItem {
                id: "b".into(),
                label: "tmux: attach".into(),
                detail: None,
            },
            QuickPickItem {
                id: "c".into(),
                label: "close tab".into(),
                detail: None,
            },
        ];
        let filtered = filter_items(&items, "tab");
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "a");
        assert_eq!(filtered[1].id, "c");
    }

    #[test]
    fn empty_query_keeps_all_items() {
        let items = vec![QuickPickItem {
            id: "1".into(),
            label: "x".into(),
            detail: Some("detail".into()),
        }];
        assert_eq!(filter_items(&items, "").len(), 1);
    }

    #[test]
    fn empty_item_list_stays_empty() {
        assert!(filter_items(&[], "anything").is_empty());
    }

    #[test]
    fn list_height_is_clamped() {
        let (panel, list) = panel_list_heights(900);
        assert_eq!(panel, 420);
        assert_eq!(list, panel - ENTRY_HEIGHT - 8);
        assert!(list <= panel);

        let (panel2, list2) = panel_list_heights(100);
        assert_eq!(panel2, 200);
        assert_eq!(list2, 156);
        assert!(list2 <= panel2);
        assert!(list2 >= 100);
    }

    #[test]
    fn filter_matches_detail() {
        let items = vec![QuickPickItem {
            id: "s".into(),
            label: "session".into(),
            detail: Some("main · 2 windows".into()),
        }];
        assert_eq!(filter_items(&items, "windows").len(), 1);
    }

    #[test]
    fn freeform_filter_prepends_typed_value() {
        let presets = vec![QuickPickItem {
            id: "cfg".into(),
            label: "alice@box:22".into(),
            detail: Some("from config".into()),
        }];
        let filtered = freeform_filter(&presets, "bob@h");
        assert_eq!(filtered[0].id, FREEFORM_ID);
        assert_eq!(filtered[0].label, "bob@h");
        assert_eq!(filtered.len(), 1);

        let filtered = freeform_filter(&presets, "alice");
        assert_eq!(filtered[0].id, FREEFORM_ID);
        assert!(filtered.iter().any(|item| item.id == "cfg"));

        let empty_query = freeform_filter(&presets, "");
        assert_eq!(empty_query.len(), 1);
        assert_eq!(empty_query[0].id, "cfg");
    }
}
