import Foundation

/// GitHub issue 报告文本：脱敏 + 只保留末尾，避免把整份调试日志塞进 issue。
///
/// 生产入口由 About 窗口在后台队列调用；这里保持纯函数，方便单测。
public enum MuxtermIssueReport {
    public static let repository = "wlingze/muxterm"

    public static func redact(_ text: String) -> String {
        text.split(separator: "\n", omittingEmptySubsequences: false).map { line in
            let lower = line.lowercased()
            let sensitive = [
                "password", "secret", "token", "api_key", "apikey",
                "authorization", "private_key", "begin openssh private",
            ]
            if sensitive.contains(where: { lower.contains($0) }) {
                return "[redacted]"
            }
            return String(line)
        }.joined(separator: "\n")
    }

    /// 只保留末尾，避免把整份调试日志塞进 issue。
    ///
    /// 直接切 UTF-8 view，不 `Array(text.utf8)`：debug 日志可能到几百 MB，
    /// 全量拷贝会在点击「Submit GitHub issue」时把主线程卡住。
    public static func tail(_ text: String, maxBytes: Int) -> String {
        guard maxBytes > 0 else { return "" }
        let utf8 = text.utf8
        guard utf8.count > maxBytes else { return redact(text) }
        var start = utf8.index(utf8.endIndex, offsetBy: -maxBytes)
        // 不能从多字节字符中间切开。
        while start < utf8.endIndex, utf8[start] & 0b1100_0000 == 0b1000_0000 {
            start = utf8.index(after: start)
        }
        return redact(String(decoding: utf8[start...], as: UTF8.self))
    }

    /// 只读日志末尾 `maxBytes` 字节，不把整份日志读进内存。
    ///
    /// 旧实现 `FileManager.contents(atPath:)` + 全量 String 会随日志增长
    /// 线性吃掉内存和主线程时间；debug 日志一天可以到几百 MB。
    public static func readLogTail(path: String, maxBytes: Int) -> String {
        guard maxBytes > 0, !path.isEmpty else { return "" }
        guard let handle = FileHandle(forReadingAtPath: path) else { return "" }
        defer { try? handle.close() }
        let size = (try? handle.seekToEnd()) ?? 0
        let start = size > UInt64(maxBytes) ? size - UInt64(maxBytes) : 0
        guard (try? handle.seek(toOffset: start)) != nil else { return "" }
        let data = (try? handle.readToEnd()) ?? Data()
        return tail(String(decoding: data, as: UTF8.self), maxBytes: maxBytes)
    }

    public static func githubIssueURL(title: String, body: String) -> URL? {
        var components = URLComponents(string: "https://github.com/\(repository)/issues/new")
        components?.queryItems = [
            URLQueryItem(name: "title", value: title),
            URLQueryItem(name: "body", value: body),
        ]
        return components?.url
    }
}
