import Foundation
import Testing
@testable import SyncStewardMac

@Test
func overviewPayloadDecodesSyncStewardOverviewShape() throws {
    let json = #"""
    {
      "config_source": "default config /Users/johndeaton/.config/syncsteward/config.toml",
      "generated_at_unix_ms": 1774837279798,
      "preflight_ready": true,
      "failing_check_count": 0,
      "warning_check_count": 1,
      "active_alert_count": 0,
      "runner": {
        "cycle_interval_minutes": 60,
        "tick_interval_minutes": 15,
        "due": false,
        "last_live_cycle_finished_at_unix_ms": 1774835105570,
        "next_due_at_unix_ms": 1774838705570,
        "last_cycle": {
          "finished_at_unix_ms": 1774835105570,
          "outcome": "success",
          "summary": "cycle succeeded for 14 approved targets (0 active alerts)"
        },
        "last_tick": {
          "finished_at_unix_ms": 1774836783702,
          "outcome": "no_op",
          "summary": "runner tick skipped cycle because it is not due (0 active alerts)"
        }
      },
      "targets": {
        "total_target_count": 23,
        "managed_target_count": 10,
        "approved_target_count": 14,
        "ready_approved_target_count": 14,
        "blocked_target_count": 9,
        "live_success_target_count": 14
      },
      "approved_targets": [
        {
          "selector": ".memloft",
          "resolved": true,
          "detail": "approved target is ready",
          "evaluation": {
            "target": {
              "target_id": null,
              "name": ".memloft",
              "local_path": "/Users/johndeaton/.memloft",
              "remote_path": "OneDrive/.memloft"
            },
            "effective_mode": "backup_only",
            "ready": true,
            "blockers": []
          },
          "last_run": {
            "target_name": ".memloft",
            "target_id": null,
            "local_path": "/Users/johndeaton/.memloft",
            "outcome": "success",
            "finished_at_unix_ms": 1774835061648,
            "summary": "run succeeded for .memloft"
          }
        }
      ],
      "recent_target_runs": [
        {
          "target_name": ".memloft",
          "target_id": null,
          "local_path": "/Users/johndeaton/.memloft",
          "outcome": "success",
          "finished_at_unix_ms": 1774835061648,
          "summary": "run succeeded for .memloft"
        }
      ],
      "alerts": []
    }
    """#

    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    let payload = try decoder.decode(OverviewPayload.self, from: Data(json.utf8))

    #expect(payload.preflightReady)
    #expect(payload.runner.lastTick?.outcome == .noOp)
    #expect(payload.approvedTargets.first?.displayName == ".memloft")
    #expect(payload.recentTargetRuns.first?.outcome == .success)
}
