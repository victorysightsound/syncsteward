import AppKit
import Foundation
import SwiftUI

@MainActor
final class OverviewStore: ObservableObject {
    @Published private(set) var overview: OverviewPayload?
    @Published private(set) var errorMessage: String?
    @Published private(set) var isLoading = false
    @Published private(set) var lastRefreshDate: Date?

    let configPath = URL(fileURLWithPath: NSHomeDirectory()).appending(path: ".config/syncsteward/config.toml")
    let stateFolderURL = URL(fileURLWithPath: NSHomeDirectory()).appending(path: ".local/state/syncsteward")

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
            let overview = try cli.fetchOverview()
            self.overview = overview
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
}

struct SyncStewardCLI {
    private let fileManager = FileManager.default

    var displayPath: String {
        resolvedCommandDescription()
    }

    func fetchOverview() throws -> OverviewPayload {
        let result = try run(arguments: ["overview", "--json"])
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(OverviewPayload.self, from: result)
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

    private func resolvedCLIExecutablePath() -> String? {
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
}

enum SyncStewardCLIError: LocalizedError {
    case commandFailed(message: String)

    var errorDescription: String? {
        switch self {
        case .commandFailed(let message):
            return message
        }
    }
}
