import Foundation

/// 新建/编辑 Project 时 runtime / transport 单选卡的纯状态模型。
public struct TargetOptionSelection {
    public var runtime: TargetRuntime
    public var transport: TargetTransport

    public init(
        runtime: TargetRuntime = .tmux,
        transport: TargetTransport = .local
    ) {
        self.runtime = runtime
        self.transport = transport
    }

    public func isSelected(runtime candidate: TargetRuntime) -> Bool {
        runtime == candidate
    }

    public func isSelected(transport candidate: TargetTransport) -> Bool {
        transport == candidate
    }

    public mutating func selectRuntime(_ candidate: TargetRuntime) {
        runtime = candidate
    }

    public mutating func selectTransport(_ candidate: TargetTransport) {
        transport = candidate
    }
}

/// 选项卡的稳定 accessibility identifier：显式暴露 selected / unselected。
public enum TargetOptionAccessibility {
    public static func identifier(kind: String, option: String, selected: Bool) -> String {
        "muxterm.target.\(kind).\(option).\(selected ? "selected" : "unselected")"
    }
}
