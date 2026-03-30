import SwiftUI

struct OverviewPayload: Decodable {
    let configSource: String
    let generatedAtUnixMs: UInt64
    let preflightReady: Bool
    let failingCheckCount: Int
    let warningCheckCount: Int
    let activeAlertCount: Int
    let runner: RunnerOverviewPayload
    let targets: TargetHealthOverviewPayload
    let approvedTargets: [ApprovedTargetOverviewPayload]
    let recentTargetRuns: [RecentTargetRunSummaryPayload]
    let alerts: [AlertRecordPayload]
}

struct RunnerOverviewPayload: Decodable {
    let cycleIntervalMinutes: Int
    let tickIntervalMinutes: Int
    let due: Bool
    let lastLiveCycleFinishedAtUnixMs: UInt64?
    let nextDueAtUnixMs: UInt64?
    let lastCycle: RunnerCycleSummaryPayload?
    let lastTick: RunnerTickSummaryPayload?
}

struct RunnerCycleSummaryPayload: Decodable {
    let finishedAtUnixMs: UInt64
    let outcome: OutcomeValue
    let summary: String
}

struct RunnerTickSummaryPayload: Decodable {
    let finishedAtUnixMs: UInt64
    let outcome: OutcomeValue
    let summary: String
}

struct TargetHealthOverviewPayload: Decodable {
    let totalTargetCount: Int
    let managedTargetCount: Int
    let approvedTargetCount: Int
    let readyApprovedTargetCount: Int
    let blockedTargetCount: Int
    let liveSuccessTargetCount: Int
}

struct ApprovedTargetOverviewPayload: Decodable {
    let selector: String
    let resolved: Bool
    let detail: String
    let evaluation: TargetEvaluationPayload?
    let lastRun: RecentTargetRunSummaryPayload?

    var displayName: String {
        evaluation?.target.name ?? selector
    }

    var stateLabel: String {
        if !resolved {
            return "UNRESOLVED"
        }
        return evaluation?.ready == true ? "READY" : "BLOCKED"
    }

    var stateColor: Color {
        if !resolved {
            return .orange
        }
        return evaluation?.ready == true ? .green : .red
    }
}

struct TargetEvaluationPayload: Decodable {
    let target: SyncTargetRecordPayload
    let effectiveMode: String
    let ready: Bool
    let blockers: [TargetBlockerPayload]
}

struct SyncTargetRecordPayload: Decodable {
    let targetId: String?
    let name: String
    let localPath: String
    let remotePath: String
}

struct TargetBlockerPayload: Decodable {
    let summary: String
    let detail: String
}

struct RecentTargetRunSummaryPayload: Decodable {
    let targetName: String
    let targetId: String?
    let localPath: String
    let outcome: OutcomeValue
    let finishedAtUnixMs: UInt64
    let summary: String
}

struct AlertRecordPayload: Decodable {
    let severity: AlertSeverityValue
    let summary: String
    let detail: String
}

struct RunnerAgentStatusEnvelope: Decodable {
    let configSource: String
    let status: LaunchAgentStatusPayload
}

struct LaunchAgentStatusPayload: Decodable {
    let label: String
    let plistPath: String
    let installed: Bool
    let loaded: Bool
    let running: Bool
    let detail: String

    var stateLabel: String {
        if running {
            return "RUNNING"
        }
        if loaded {
            return "LOADED"
        }
        if installed {
            return "INSTALLED"
        }
        return "MISSING"
    }

    var stateColor: Color {
        if running {
            return .green
        }
        if loaded {
            return .blue
        }
        if installed {
            return .orange
        }
        return .red
    }

    var detailLine: String {
        let trimmed = detail.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            return plistPath
        }
        return trimmed
    }
}

struct RunnerTickActionPayload: Decodable {
    let dryRun: Bool
    let outcome: OutcomeValue
    let summary: String
    let due: Bool
    let preflightReady: Bool
}

enum AlertSeverityValue: String, Decodable {
    case info
    case warn
    case critical

    var color: Color {
        switch self {
        case .info:
            return .blue
        case .warn:
            return .orange
        case .critical:
            return .red
        }
    }
}

enum OutcomeValue: String, Decodable {
    case success = "success"
    case noOp = "no_op"
    case blocked = "blocked"
    case failed = "failed"

    var color: Color {
        switch self {
        case .success:
            return .green
        case .noOp:
            return .blue
        case .blocked:
            return .orange
        case .failed:
            return .red
        }
    }
}

extension UInt64 {
    func formattedUnixMillis() -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(self) / 1000)
        return date.formatted(date: .abbreviated, time: .shortened)
    }
}
