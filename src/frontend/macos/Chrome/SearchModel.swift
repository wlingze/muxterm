import Foundation

/// 一次搜索命中（core `muxterm_search_all` JSON 的 Swift 视图）。
public struct SearchHit: Equatable, Sendable {
    public let workspaceId: String
    public let tabId: UInt32
    public let paneId: UInt32
    public let seq: UInt64
    public let line: String

    public init(
        workspaceId: String,
        tabId: UInt32,
        paneId: UInt32,
        seq: UInt64,
        line: String
    ) {
        self.workspaceId = workspaceId
        self.tabId = tabId
        self.paneId = paneId
        self.seq = seq
        self.line = line
    }
}

/// 搜索快照（core `muxterm_search_all` JSON 的 Swift 视图）。
public struct SearchSnapshot: Equatable, Sendable {
    public let hits: [SearchHit]

    public init(hits: [SearchHit]) {
        self.hits = hits
    }

    /// 解析 core JSON；失败返回 nil（不 panic）。
    public static func decode(_ data: Data) -> SearchSnapshot? {
        guard let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let ok = json["ok"] as? Bool, ok
        else {
            return nil
        }
        let hits: [SearchHit] = (json["hits"] as? [[String: Any]])?
            .compactMap { h in
                guard let workspaceId = h["workspace_id"] as? String,
                      let tabId = JSONNumber.uint32(h["tab_id"]),
                      let paneId = JSONNumber.uint32(h["pane_id"])
                else {
                    return nil
                }
                return SearchHit(
                    workspaceId: workspaceId,
                    tabId: tabId,
                    paneId: paneId,
                    seq: JSONNumber.uint64(h["seq"]) ?? 0,
                    line: (h["line"] as? String) ?? ""
                )
            } ?? []
        return SearchSnapshot(hits: hits)
    }
}

/// 搜索列表纯逻辑：按 query 过滤命中行，空结果标记占位。
public enum SearchList {
    public static func rows(from snapshot: SearchSnapshot, query: String) -> (rows: [SearchHit], isEmpty: Bool) {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let rows = snapshot.hits.filter { h in
            q.isEmpty
                || h.line.lowercased().contains(q)
                || h.workspaceId.lowercased().contains(q)
        }
        return (rows, rows.isEmpty)
    }
}

/// JSONSerialization 数字可能是 `NSNumber` / `Int` / `UInt32`；tab_id==0 也是合法 tab。
enum JSONNumber {
    static func uint32(_ value: Any?) -> UInt32? {
        if let n = value as? UInt32 { return n }
        if let n = value as? Int, n >= 0 { return UInt32(n) }
        if let n = value as? Int64, n >= 0 { return UInt32(clamping: n) }
        if let n = value as? NSNumber { return n.uint32Value }
        return nil
    }

    static func uint64(_ value: Any?) -> UInt64? {
        if let n = value as? UInt64 { return n }
        if let n = value as? Int, n >= 0 { return UInt64(n) }
        if let n = value as? Int64, n >= 0 { return UInt64(n) }
        if let n = value as? NSNumber { return n.uint64Value }
        return nil
    }
}
