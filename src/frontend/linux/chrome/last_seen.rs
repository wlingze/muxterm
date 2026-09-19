//! 历史提示只保存稳定行号，不保存重复文本或依赖 pane 的局部数字身份。
use std::collections::HashMap;
use std::time::{Duration, Instant};

type PaneKey = (String, u32);

#[derive(Default)]
pub struct LastSeen {
    current: Option<(PaneKey, Option<u64>)>,
    baselines: HashMap<PaneKey, u64>,
    offer: Option<(PaneKey, u32, Instant, Instant)>,
}

impl LastSeen {
    pub fn observe(&mut self, key: PaneKey, latest: Option<u64>) {
        let latest = latest.filter(|seq| *seq > 0);
        if self.current.as_ref().is_some_and(|(old, _)| old != &key) {
            if let Some((old, Some(seq))) = self.current.take() {
                self.baselines.insert(old, seq);
            }
            self.offer = None;
        }
        self.current = Some((key, latest));
    }

    pub fn baseline(&self, key: &PaneKey) -> Option<u64> {
        self.baselines.get(key).copied()
    }

    pub fn target(&mut self, key: &PaneKey, offset: Option<u32>, now: Instant) -> Option<u32> {
        if let Some((owner, _, deadline, _)) = &self.offer {
            if owner == key && now >= *deadline {
                self.dismiss(key);
                return None;
            }
        }
        let progressed = self.current.as_ref().is_some_and(|(owner, latest)| {
            owner == key && latest.zip(self.baseline(key)).is_some_and(|(a, b)| a > b)
        });
        if !progressed {
            self.offer = None;
            return None;
        }
        if let Some(offset) = offset {
            let deadline = self
                .offer
                .as_ref()
                .filter(|(owner, _, _, _)| owner == key)
                .map(|(_, _, deadline, _)| *deadline)
                .unwrap_or(now + Duration::from_secs(4));
            self.offer = Some((key.clone(), offset, deadline, now));
            return Some(offset);
        }
        self.offer
            .as_ref()
            .filter(|(owner, _, _, valid)| {
                owner == key && now.duration_since(*valid) <= Duration::from_secs(1)
            })
            .map(|(_, offset, _, _)| *offset)
    }

    pub fn dismiss(&mut self, key: &PaneKey) {
        self.baselines.remove(key);
        self.offer = None;
    }

    pub fn remove_workspace(&mut self, workspace: &str) {
        self.baselines.retain(|(owner, _), _| owner != workspace);
        if self
            .current
            .as_ref()
            .is_some_and(|((owner, _), _)| owner == workspace)
        {
            self.current = None;
            self.offer = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_offer_expires_without_rearming_and_scopes_duplicate_panes() {
        let mut model = LastSeen::default();
        let a = ("local".into(), 1);
        let b = ("ssh".into(), 1);
        let now = Instant::now();
        model.observe(a.clone(), Some(10));
        model.observe(b.clone(), Some(100));
        assert_eq!(model.baseline(&a), Some(10));
        assert_eq!(model.baseline(&b), None);
        model.observe(a.clone(), Some(20));
        assert_eq!(model.target(&a, Some(5), now), Some(5));
        assert_eq!(
            model.target(&a, None, now + Duration::from_millis(500)),
            Some(5)
        );
        assert_eq!(
            model.target(&a, Some(7), now + Duration::from_secs(4)),
            None
        );
        assert_eq!(
            model.target(&a, Some(7), now + Duration::from_secs(5)),
            None
        );
    }

    #[test]
    fn zero_and_unchanged_sequences_do_not_offer_navigation() {
        let mut model = LastSeen::default();
        let a = ("local".into(), 1);
        let b = ("local".into(), 2);
        model.observe(a.clone(), Some(0));
        model.observe(b.clone(), Some(10));
        assert_eq!(model.baseline(&a), None);
        model.observe(a, Some(20));
        model.observe(b.clone(), Some(10));
        assert_eq!(model.target(&b, Some(0), Instant::now()), None);
        model.remove_workspace("local");
        assert_eq!(model.baseline(&b), None);
    }
}
