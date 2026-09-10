//! Status bar data owned outside the GTK widget tree.

/// Connection summary displayed by the status popover.
#[derive(Debug, Clone, Default)]
pub struct ConnectionSummary {
    pub kind: String,
    pub host: Option<String>,
    pub status: String,
    /// 累计下行字节（SSH transport 读端）。
    pub down: u64,
    /// 累计上行字节（SSH PtyWriter 写端）。
    pub up: u64,
    /// 瞬时下行字节/秒（由连续两次 snapshot 差出来，不是累计）。
    pub down_rate: u64,
    /// 瞬时上行字节/秒。
    pub up_rate: u64,
}
