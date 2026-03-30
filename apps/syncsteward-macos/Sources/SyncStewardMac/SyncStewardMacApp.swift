import AppKit
import SwiftUI

@MainActor
final class DashboardWindowController: NSObject, NSWindowDelegate {
    static let shared = DashboardWindowController()

    private var store: OverviewStore?
    private var window: NSWindow?

    func configure(store: OverviewStore) {
        self.store = store
    }

    func show() {
        guard let store else { return }

        if let hostingController = window?.contentViewController as? NSHostingController<SyncStewardControlCenterView> {
            hostingController.rootView = SyncStewardControlCenterView(store: store)
        } else {
            let hostingController = NSHostingController(rootView: SyncStewardControlCenterView(store: store))
            let dashboardWindow = NSWindow(contentViewController: hostingController)
            dashboardWindow.title = "SyncSteward"
            dashboardWindow.setContentSize(NSSize(width: 560, height: 680))
            dashboardWindow.minSize = NSSize(width: 520, height: 640)
            dashboardWindow.styleMask = [.titled, .closable, .miniaturizable, .resizable]
            dashboardWindow.isReleasedWhenClosed = false
            dashboardWindow.delegate = self
            window = dashboardWindow
        }

        NSApplication.shared.activate(ignoringOtherApps: true)
        window?.center()
        window?.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        NSApplication.shared.hide(nil)
    }
}

@MainActor
final class SyncStewardAppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.15) {
            DashboardWindowController.shared.show()
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        DashboardWindowController.shared.show()
        return true
    }
}

@main
struct SyncStewardMacApp: App {
    @NSApplicationDelegateAdaptor(SyncStewardAppDelegate.self) private var appDelegate
    @StateObject private var store: OverviewStore

    init() {
        let store = OverviewStore()
        _store = StateObject(wrappedValue: store)
        DashboardWindowController.shared.configure(store: store)
    }

    var body: some Scene {
        MenuBarExtra("SyncSteward", systemImage: store.statusSymbolName) {
            SyncStewardMenuBarView(store: store)
                .frame(width: 420)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SyncStewardSettingsView(store: store)
                .frame(width: 420, height: 220)
        }
    }
}

struct SyncStewardControlCenterView: View {
    @ObservedObject var store: OverviewStore

    var body: some View {
        ScrollView {
            SyncStewardMenuBarView(store: store, includeOpenWindowAction: false)
                .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .task {
            await store.refreshIfNeeded()
        }
    }
}

struct SyncStewardMenuBarView: View {
    @ObservedObject var store: OverviewStore
    var includeOpenWindowAction: Bool = true

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            header

            if let overview = store.overview {
                summaryCards(overview: overview)

                if let runnerAgentStatus = store.runnerAgentStatus {
                    runnerAgentSection(status: runnerAgentStatus)
                }

                if !overview.approvedTargets.isEmpty {
                    approvedTargetsSection(overview: overview)
                }

                if !overview.alerts.isEmpty {
                    alertsSection(overview: overview)
                }

                if !overview.recentTargetRuns.isEmpty {
                    recentRunsSection(overview: overview)
                }
            } else if store.isLoading {
                ProgressView("Loading SyncSteward overview...")
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 24)
            } else {
                Text("No overview loaded yet.")
                    .foregroundStyle(.secondary)
            }

            if let errorMessage = store.errorMessage {
                errorBanner(errorMessage)
            }

            if let actionFeedback = store.actionFeedback {
                actionBanner(actionFeedback)
            }

            Divider()
            actionSection
        }
        .padding(16)
        .task {
            await store.refreshIfNeeded()
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 12) {
            Circle()
                .fill(store.statusColor)
                .frame(width: 12, height: 12)
                .padding(.top, 4)

            VStack(alignment: .leading, spacing: 4) {
                Text("SyncSteward")
                    .font(.system(size: 16, weight: .semibold, design: .rounded))
                Text(store.statusLine)
                    .font(.system(size: 12, weight: .medium, design: .rounded))
                    .foregroundStyle(.secondary)
                if let refreshed = store.lastRefreshLabel {
                    Text("Updated \(refreshed)")
                        .font(.system(size: 11, weight: .regular, design: .rounded))
                        .foregroundStyle(.tertiary)
                }
            }

            Spacer()
        }
    }

    private func summaryCards(overview: OverviewPayload) -> some View {
        VStack(spacing: 10) {
            HStack(spacing: 10) {
                SummaryCard(
                    title: "Preflight",
                    value: overview.preflightReady ? "Ready" : "Blocked",
                    detail: "\(overview.failingCheckCount) failing / \(overview.warningCheckCount) warnings",
                    tint: overview.preflightReady ? .green : .red
                )
                SummaryCard(
                    title: "Alerts",
                    value: "\(overview.activeAlertCount)",
                    detail: overview.activeAlertCount == 0 ? "clear" : "active",
                    tint: overview.activeAlertCount == 0 ? .green : .orange
                )
            }

            HStack(spacing: 10) {
                SummaryCard(
                    title: "Runner",
                    value: overview.runner.due ? "Due" : "Idle",
                    detail: "every \(overview.runner.cycleIntervalMinutes)m",
                    tint: overview.runner.due ? .orange : .blue
                )
                SummaryCard(
                    title: "Approved",
                    value: "\(overview.targets.readyApprovedTargetCount)/\(overview.targets.approvedTargetCount)",
                    detail: "ready / configured",
                    tint: overview.targets.readyApprovedTargetCount == overview.targets.approvedTargetCount ? .green : .orange
                )
            }
        }
    }

    private func runnerAgentSection(status: LaunchAgentStatusPayload) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionHeader("Runner Agent")
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text(status.label)
                        .font(.system(size: 12, weight: .semibold, design: .rounded))
                    Text(status.detailLine)
                        .font(.system(size: 11, weight: .regular, design: .rounded))
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Text(status.stateLabel)
                    .font(.system(size: 10, weight: .bold, design: .rounded))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(status.stateColor.opacity(0.16), in: Capsule())
                    .foregroundStyle(status.stateColor)
            }
            .padding(10)
            .background(Color(NSColor.controlBackgroundColor), in: RoundedRectangle(cornerRadius: 10))
        }
    }

    private func approvedTargetsSection(overview: OverviewPayload) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionHeader("Approved Targets")
            ForEach(Array(overview.approvedTargets.prefix(8).enumerated()), id: \.offset) { _, target in
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(target.displayName)
                            .font(.system(size: 12, weight: .semibold, design: .rounded))
                        Spacer()
                        Text(target.stateLabel)
                            .font(.system(size: 10, weight: .bold, design: .rounded))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 4)
                            .background(target.stateColor.opacity(0.16), in: Capsule())
                            .foregroundStyle(target.stateColor)
                    }
                    Text(target.detail)
                        .font(.system(size: 11, weight: .regular, design: .rounded))
                        .foregroundStyle(.secondary)
                    if let lastRun = target.lastRun {
                        Text("Last run: \(lastRun.summary)")
                            .font(.system(size: 11, weight: .regular, design: .rounded))
                            .foregroundStyle(.tertiary)
                    }
                }
                .padding(10)
                .background(Color(NSColor.controlBackgroundColor), in: RoundedRectangle(cornerRadius: 10))
            }

            if overview.approvedTargets.count > 8 {
                Text("Showing 8 of \(overview.approvedTargets.count) approved targets.")
                    .font(.system(size: 11, weight: .regular, design: .rounded))
                    .foregroundStyle(.tertiary)
            }
        }
    }

    private func alertsSection(overview: OverviewPayload) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionHeader("Alerts")
            ForEach(Array(overview.alerts.prefix(4).enumerated()), id: \.offset) { _, alert in
                VStack(alignment: .leading, spacing: 4) {
                    Text(alert.summary)
                        .font(.system(size: 12, weight: .semibold, design: .rounded))
                    Text(alert.detail)
                        .font(.system(size: 11, weight: .regular, design: .rounded))
                        .foregroundStyle(.secondary)
                }
                .padding(10)
                .background(alert.severity.color.opacity(0.14), in: RoundedRectangle(cornerRadius: 10))
            }
        }
    }

    private func recentRunsSection(overview: OverviewPayload) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionHeader("Recent Runs")
            ForEach(Array(overview.recentTargetRuns.prefix(5).enumerated()), id: \.offset) { _, run in
                HStack(alignment: .top, spacing: 10) {
                    Circle()
                        .fill(run.outcome.color)
                        .frame(width: 8, height: 8)
                        .padding(.top, 5)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(run.targetName)
                            .font(.system(size: 12, weight: .semibold, design: .rounded))
                        Text(run.summary)
                            .font(.system(size: 11, weight: .regular, design: .rounded))
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Text(run.finishedAtUnixMs.formattedUnixMillis())
                        .font(.system(size: 10, weight: .regular, design: .rounded))
                        .foregroundStyle(.tertiary)
                }
            }
        }
    }

    private func errorBanner(_ message: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("SyncSteward UI could not refresh")
                .font(.system(size: 12, weight: .semibold, design: .rounded))
            Text(message)
                .font(.system(size: 11, weight: .regular, design: .rounded))
                .foregroundStyle(.secondary)
        }
        .padding(10)
        .background(Color.red.opacity(0.14), in: RoundedRectangle(cornerRadius: 10))
    }

    private func actionBanner(_ feedback: ActionFeedback) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(feedback.title)
                .font(.system(size: 12, weight: .semibold, design: .rounded))
            Text(feedback.message)
                .font(.system(size: 11, weight: .regular, design: .rounded))
                .foregroundStyle(.secondary)
        }
        .padding(10)
        .background(feedback.tone.color.opacity(0.14), in: RoundedRectangle(cornerRadius: 10))
    }

    private var actionSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            sectionHeader("Operator")

            HStack(spacing: 10) {
                Button(store.isLoading ? "Refreshing..." : "Refresh") {
                    Task {
                        await store.refresh()
                    }
                }
                .disabled(store.isLoading || store.isPerformingAction)

                Button(store.isPerformingAction ? "Running..." : "Dry-Run Tick") {
                    Task {
                        await store.runDryRunTick()
                    }
                }
                .disabled(store.isLoading || store.isPerformingAction)

                Spacer()
            }

            HStack(spacing: 10) {
                if includeOpenWindowAction {
                    Button("Open Dashboard") {
                        DashboardWindowController.shared.show()
                    }
                }

                Button("Open Config") {
                    store.openConfig()
                }

                Button("Open State") {
                    store.openStateFolder()
                }

                Button("Open Logs") {
                    store.openRunnerLogs()
                }

                Button("Open Audit") {
                    store.openAuditLog()
                }
            }

            HStack {
                Spacer()
                Button("Quit") {
                    NSApplication.shared.terminate(nil)
                }
            }
        }
    }

    private func sectionHeader(_ title: String) -> some View {
        Text(title)
            .font(.system(size: 11, weight: .bold, design: .rounded))
            .foregroundStyle(.secondary)
            .textCase(.uppercase)
    }
}

struct SyncStewardSettingsView: View {
    @ObservedObject var store: OverviewStore

    var body: some View {
        Form {
            LabeledContent("CLI Path") {
                Text(store.cliDisplayPath)
                    .multilineTextAlignment(.trailing)
                    .foregroundStyle(.secondary)
            }
            LabeledContent("Config") {
                Text(store.configPath.path)
                    .multilineTextAlignment(.trailing)
                    .foregroundStyle(.secondary)
            }
            LabeledContent("State Folder") {
                Text(store.stateFolderURL.path)
                    .multilineTextAlignment(.trailing)
                    .foregroundStyle(.secondary)
            }
            LabeledContent("Refresh") {
                Text("60 seconds")
                    .foregroundStyle(.secondary)
            }
            HStack {
                Button("Refresh Now") {
                    Task {
                        await store.refresh()
                    }
                }
                Spacer()
                Button("Open Config") {
                    store.openConfig()
                }
            }
        }
        .padding(16)
    }
}

struct SummaryCard: View {
    let title: String
    let value: String
    let detail: String
    let tint: Color

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.system(size: 11, weight: .bold, design: .rounded))
                .foregroundStyle(.secondary)
                .textCase(.uppercase)
            Text(value)
                .font(.system(size: 20, weight: .semibold, design: .rounded))
            Text(detail)
                .font(.system(size: 11, weight: .regular, design: .rounded))
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .background(tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 12))
    }
}
