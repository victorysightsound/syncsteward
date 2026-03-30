import AppKit
import Foundation
import SwiftUI

@MainActor
final class OverviewStore: ObservableObject {
    @Published private(set) var overview: OverviewPayload?
    @Published private(set) var runnerAgentStatus: LaunchAgentStatusPayload?
    @Published private(set) var errorMessage: String?
    @Published private(set) var actionFeedback: ActionFeedback?
    @Published private(set) var isLoading = false
    @Published private(set) var isPerformingAction = false
    @Published private(set) var lastRefreshDate: Date?

    let configPath = URL(fileURLWithPath: NSHomeDirectory()).appending(path: ".config/syncsteward/config.toml")
    let stateFolderURL = URL(fileURLWithPath: NSHomeDirectory()).appending(path: ".local/state/syncsteward")
    let runnerStdoutURL = URL(fileURLWithPath: NSHomeDirectory()).appending(path: ".local/state/syncsteward/runner.stdout.log")
    let runnerStderrURL = URL(fileURLWithPath: NSHomeDirectory()).appending(path: ".local/state/syncsteward/runner.stderr.log")
    let auditLogURL = URL(fileURLWithPath: NSHomeDirectory()).appending(path: ".local/state/syncsteward/audit.jsonl")

    private let refreshInterval: TimeInterval = 60
    private var refreshTimer: Timer?
    private let cli = SyncStewardCLI()

    init() {
        refreshTimer = Timer.scheduledTimer(withTimeInterval: refreshInterval, repeats: true) { [weak self] _ in
            guard let self else { return }
            Task { await self.refresh() }
        }
    }

    var statusColor: Color {
        guard let overview else { return .gray }
        if !overview.preflightReady || overview.activeAlertCount > 0 {
            return .red
        }
        if overview.warningCheckCount > 0 {
            return .orange
        }
        return .green
    }

    var statusSymbolName: String {
        guard let overview else { return "arrow.triangle.2.circlepath.circle" }
        if !overview.preflightReady || overview.activeAlertCount > 0 {
            return "exclamationmark.triangle.fill"
        }
        if overview.warningCheckCount > 0 {
            return "exclamationmark.circle.fill"
        }
        return "checkmark.circle.fill"
    }

    var statusLine: String {
        guard let overview else { return "Waiting for SyncSteward..." }
        if !overview.preflightReady {
            return "Preflight is blocked"
        }
        if overview.activeAlertCount > 0 {
            return "\(overview.activeAlertCount) active alerts"
        }
        if overview.warningCheckCount > 0 {
            return "Healthy with acknowledged warnings"
        }
        return "Healthy approved target set"
    }

    var lastRefreshLabel: String? {
        guard let lastRefreshDate else { return nil }
        return lastRefreshDate.formatted(date: .omitted, time: .shortened)
    }

    var cliDisplayPath: String {
        cli.displayPath
    }

    func refreshIfNeeded() async {
        if overview == nil && !isLoading {
            await refresh()
        }
    }

    func refresh() async {
        isLoading = true
        defer { isLoading = false }

        do {
            let cli = self.cli
            let overviewTask = Task.detached(priority: .userInitiated) {
                try cli.fetchOverview()
            }
            let runnerTask = Task.detached(priority: .utility) {
                try cli.fetchRunnerAgentStatus()
            }

            let overview = try await overviewTask.value
            let runnerAgentStatus = try? await runnerTask.value
            self.overview = overview
            self.runnerAgentStatus = runnerAgentStatus
            self.errorMessage = nil
            self.lastRefreshDate = Date()
        } catch {
            self.errorMessage = error.localizedDescription
        }
    }

    func openConfig() {
        NSWorkspace.shared.open(configPath)
    }

    func openStateFolder() {
        NSWorkspace.shared.activateFileViewerSelecting([stateFolderURL])
    }

    func openRunnerLogs() {
        NSWorkspace.shared.activateFileViewerSelecting([runnerStdoutURL, runnerStderrURL])
    }

    func openAuditLog() {
        NSWorkspace.shared.open(auditLogURL)
    }

    func runDryRunTick() async {
        await performAction(title: "Dry-run tick complete") { cli in
            try cli.runDryRunTick()
        }
    }

    private func performAction(
        title: String,
        operation: @escaping @Sendable (SyncStewardCLI) throws -> RunnerTickActionPayload
    ) async {
        isPerformingAction = true
        defer { isPerformingAction = false }

        do {
            let cli = self.cli
            let payload = try await Task.detached(priority: .userInitiated) {
                try operation(cli)
            }.value
            actionFeedback = ActionFeedback(
                title: title,
                message: payload.summary,
                tone: payload.outcome == .failed ? .error : .info
            )
            await refresh()
        } catch {
            actionFeedback = ActionFeedback(
                title: "Operator action failed",
                message: error.localizedDescription,
                tone: .error
            )
        }
    }
}

struct SyncStewardCLI: Sendable {
    var displayPath: String {
        resolvedCommandDescription()
    }

    func fetchOverview() throws -> OverviewPayload {
        try decode(OverviewPayload.self, arguments: ["overview", "--json"])
    }

    func fetchRunnerAgentStatus() throws -> LaunchAgentStatusPayload {
        try decode(RunnerAgentStatusEnvelope.self, arguments: ["runner-agent-status", "--json"]).status
    }

    func runDryRunTick() throws -> RunnerTickActionPayload {
        try decode(RunnerTickActionPayload.self, arguments: ["runner-tick", "--dry-run", "--json"])
    }

    private func run(arguments: [String]) throws -> Data {
        let process = Process()
        let stdout = Pipe()
        let stderr = Pipe()

        if let executablePath = resolvedCLIExecutablePath() {
            process.executableURL = URL(fileURLWithPath: executablePath)
            process.arguments = arguments
        } else {
            process.executableURL = URL(fileURLWithPath: "/usr/bin/env")
            process.arguments = ["syncsteward-cli"] + arguments
        }

        process.standardOutput = stdout
        process.standardError = stderr
        try process.run()
        process.waitUntilExit()

        let output = stdout.fileHandleForReading.readDataToEndOfFile()
        let errorOutput = stderr.fileHandleForReading.readDataToEndOfFile()

        guard process.terminationStatus == 0 else {
            let message = String(data: errorOutput, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            throw SyncStewardCLIError.commandFailed(message: message?.isEmpty == false ? message! : "syncsteward-cli exited with status \(process.terminationStatus)")
        }

        return output
    }

    private func decode<T: Decodable>(_ type: T.Type, arguments: [String]) throws -> T {
        let result = try run(arguments: arguments)
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase

        do {
            return try decoder.decode(T.self, from: result)
        } catch {
            let preview = outputPreview(for: result)
            throw SyncStewardCLIError.decodeFailed(
                command: commandDescription(arguments: arguments),
                message: error.localizedDescription,
                preview: preview
            )
        }
    }

    private func resolvedCLIExecutablePath() -> String? {
        let fileManager = FileManager.default
        let candidates = [
            ProcessInfo.processInfo.environment["SYNCSTEWARD_CLI_PATH"],
            "\(NSHomeDirectory())/projects/syncsteward/target/debug/syncsteward-cli",
            "\(NSHomeDirectory())/bin/syncsteward-cli",
        ]
        .compactMap { $0 }

        return candidates.first(where: { fileManager.isExecutableFile(atPath: $0) })
    }

    private func resolvedCommandDescription() -> String {
        resolvedCLIExecutablePath() ?? "syncsteward-cli (from PATH)"
    }

    private func commandDescription(arguments: [String]) -> String {
        ([resolvedCommandDescription()] + arguments).joined(separator: " ")
    }

    private func outputPreview(for data: Data) -> String {
        let raw = String(data: data, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let text = raw?.isEmpty == false ? raw! : "<empty output>"
        return String(text.prefix(320))
    }
}

enum SyncStewardCLIError: LocalizedError {
    case commandFailed(message: String)
    case decodeFailed(command: String, message: String, preview: String)

    var errorDescription: String? {
        switch self {
        case .commandFailed(let message):
            return message
        case .decodeFailed(let command, let message, let preview):
            return "Could not decode SyncSteward output from `\(command)`: \(message)\nOutput preview: \(preview)"
        }
    }
}

struct ActionFeedback {
    let title: String
    let message: String
    let tone: FeedbackTone
}

enum FeedbackTone {
    case info
    case error

    var color: Color {
        switch self {
        case .info:
            return .blue
        case .error:
            return .red
        }
    }
}
