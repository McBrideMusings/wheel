import SwiftUI
import WheelKit

/// The orchestrator's queue (ADR-0054) as the `wheelhouse.queue` feed groups it: the badge's count and tone and the
/// groups the sheet lists. The phone reads it from the orchestrator space's Queue plugin pane, whose script holds the
/// subscription; the grouping is the feed's, never the phone's.
struct OrchestratorQueue: Equatable {
    struct Item: Equatable, Identifiable {
        let id: String
        let title: String
        let kind: String
        let priority: Int
        let from: String
        let created: String
    }
    struct Group: Equatable, Identifiable {
        let id: String
        let title: String
        let items: [Item]
    }
    let count: Int
    /// `red`, `orange` or `gray`.
    let tone: String
    let groups: [Group]
    /// wheeld's clock in Unix seconds when the value was read; ages count from it.
    let now: Double
    let error: String?

    init?(_ value: JSONValue?) {
        guard case let .object(o)? = value else { return nil }
        let badge = o["badge"]?.objectValue ?? [:]
        count = badge["count"]?.intValue ?? 0
        tone = badge["tone"]?.stringValue ?? "gray"
        now = o["now"]?.doubleValue ?? 0
        error = o["error"]?.stringValue
        groups = (o["groups"]?.arrayValue ?? []).compactMap { g in
            guard let g = g.objectValue, let id = g["id"]?.stringValue else { return nil }
            let items: [Item] = (g["items"]?.arrayValue ?? []).compactMap { i in
                guard let i = i.objectValue, let id = i["id"]?.stringValue else { return nil }
                let from = [i["space"]?.stringValue, i["agent"]?.stringValue ?? i["pane"]?.stringValue].compactMap { $0 }
                return Item(id: id, title: i["title"]?.stringValue ?? "", kind: i["kind"]?.stringValue ?? "item",
                            priority: i["priority"]?.intValue ?? 2, from: from.joined(separator: " · "),
                            created: i["created"]?.stringValue ?? "")
            }
            return Group(id: id, title: g["title"]?.stringValue ?? id, items: items)
        }
    }

    var badgeColor: Color {
        switch tone {
        case "red": .red
        case "orange": .orange
        default: .gray
        }
    }

    /// What tapping `item` says to the orchestrator: picking a queue item starts its topic (the folder's CLAUDE.md).
    static func message(for item: Item) -> String { "Let's talk about \(item.id): \(item.title)" }

    /// How long before `now` an RFC 3339 time was, as `5m`, `3h` or `2d`, rounded as the Queue pane's `time.ago` rounds;
    /// empty when it does not parse.
    func age(_ created: String) -> String {
        guard let then = ISO8601DateFormatter().date(from: created)?.timeIntervalSince1970, now > 0 else { return "" }
        let minutes = max(0, now - then) / 60
        if minutes < 60 { return "\(Int(minutes.rounded()))m" }
        if minutes < 1440 { return "\(Int((minutes / 60).rounded()))h" }
        return "\(Int((minutes / 1440).rounded()))d"
    }
}

/// Orchestrator mode's queue tray in the header: the tray symbol with a pill counting what waits on the person, gray,
/// orange or red with the most urgent of them. It holds the slot the agents eye holds in Spaces mode.
struct QueueButton: View {
    let queue: OrchestratorQueue
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: "tray")
                .overlay(alignment: .topTrailing) {
                    Text("\(queue.count)")
                        .font(.caption2.weight(.bold).monospacedDigit())
                        .foregroundStyle(.black)
                        .padding(.horizontal, 4)
                        .frame(minWidth: 16, minHeight: 16)
                        .background(queue.badgeColor, in: Capsule())
                        .offset(x: 9, y: -8)
                        .opacity(queue.count > 0 ? 1 : 0)
                }
        }
        .accessibilityLabel("Queue")
        .accessibilityValue("\(queue.count) \(queue.tone)")
        .accessibilityIdentifier("queue")
    }
}

/// The queue sheet: Needs you, Ready for review and Finished, each row its kind, where it came from, its age and its
/// title. Tapping a row sends that item's message to the orchestrator's chat and closes the sheet.
struct QueueSheet: View {
    let queue: OrchestratorQueue
    let pick: (OrchestratorQueue.Item) -> Void

    var body: some View {
        NavigationStack {
            List {
                if let error = queue.error {
                    Text(error).foregroundStyle(.red)
                }
                ForEach(queue.groups) { group in
                    Section {
                        ForEach(group.items) { item in
                            Button { pick(item) } label: { row(item) }
                                .buttonStyle(.plain)
                                .accessibilityIdentifier("queue item \(item.id)")
                        }
                    } header: {
                        Text("\(group.title) · \(group.items.count)")
                            .accessibilityIdentifier("queue group \(group.id)")
                    }
                }
            }
            .navigationTitle("Queue")
            .navigationBarTitleDisplayMode(.inline)
        }
        .accessibilityIdentifier("queue sheet")
    }

    private func row(_ item: OrchestratorQueue.Item) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 8) {
                Text(item.kind.uppercased()).font(.caption.weight(.semibold)).foregroundStyle(tone(item.kind))
                Text(item.from).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                Spacer(minLength: 0)
                Text(queue.age(item.created)).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            }
            Text(item.title).lineLimit(2)
        }
        .contentShape(Rectangle())
    }

    private func tone(_ kind: String) -> Color {
        switch kind {
        case "permission", "bug": .red
        case "review": .green
        case "done", "unconfirmed": .secondary
        default: .orange
        }
    }
}

private extension JSONValue {
    var objectValue: [String: JSONValue]? { if case let .object(o) = self { o } else { nil } }
    var arrayValue: [JSONValue]? { if case let .array(a) = self { a } else { nil } }
    var stringValue: String? { if case let .string(s) = self { s } else { nil } }
    var doubleValue: Double? { if case let .number(n) = self { n } else { nil } }
    var intValue: Int? { doubleValue.map { Int($0) } }
}
