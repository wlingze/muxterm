//! Cross-workspace activity domain.
//!
//! Attention is the first migrated activity projection. Commands, agents and
//! their lane events will join this owner as the Activity contract lands.

pub mod attention;
pub mod record;

use crate::activity::attention::clock::RealClock;
use crate::activity::attention::engine::AttentionEngine;
use crate::config::AttentionConfig;
use record::ActivityStore;

/// Activity owner for one Muxterm product session.
///
/// The attention projection remains available to the legacy FFI query surface;
/// new command and agent records share this owner and its revision watermark.
pub struct ActivityState {
    pub(crate) attention: AttentionEngine<RealClock>,
    pub(crate) records: ActivityStore,
}

impl ActivityState {
    pub(crate) fn new(config: AttentionConfig) -> Self {
        Self {
            attention: AttentionEngine::new(config, RealClock),
            records: ActivityStore::default(),
        }
    }
}
