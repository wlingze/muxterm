import Foundation

/// GitHub issue 正文的纯文本处理。日志可能含口令，提交前先截断并打码。
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
    public static func tail(_ text: String, maxBytes: Int) -> String {
        guard maxBytes > 0 else { return "" }
        let utf8 = Array(text.utf8)
        guard utf8.count > maxBytes else { return redact(text) }
        var index = utf8.count - maxBytes
        while index < utf8.count, utf8[index] & 0b1100_0000 == 0b1000_0000 {
            index += 1
        }
        let slice = String(decoding: utf8[index...], as: UTF8.self)
        return redact(slice)
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
