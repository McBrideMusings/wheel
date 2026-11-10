import Foundation
import WheelKit

extension AgentChat {
    /// How many turns the conversation holds up to and through the one reply `id` belongs to: the
    /// prompts at or before it. A turn is one prompt and what the agent did after it, which is how
    /// the daemon counts. Nil for a row before any prompt.
    public func turnCount(throughRow id: Int) -> Int? {
        var turns = 0
        for row in transcript.rows {
            if case .user = row.kind { turns += 1 }
            if row.id == id { return turns > 0 ? turns : nil }
        }
        return nil
    }

    /// The turn count to fork at for reply `id`, or nil when the reply cannot be forked from: the
    /// harness or server cannot, or its turn is still running.
    public func forkTurns(row id: Int) -> Int? {
        guard canFork(), let turns = turnCount(throughRow: id) else { return nil }
        let total = transcript.rows.reduce(0) { count, row in
            if case .user = row.kind { count + 1 } else { count }
        }
        if isBusy && turns == total { return nil }
        return turns
    }

    /// Fork the conversation at reply `id`: a new pane beside this one holds it through that reply.
    public func fork(row id: Int) {
        guard let turns = forkTurns(row: id) else { return }
        UILog.info("transcript \(name): fork at row \(id), turn \(turns)")
        sendFork(turns)
    }
}
