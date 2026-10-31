// Leaves a shared link or text for the Zunder app: one JSON file per item in
// the App Group container both share (`share-inbox/<id>.json`). The app reads
// the folder at start, on return to the foreground and every few seconds while
// it runs (apps/zunder-app/src-tauri/src/share.rs), and the web UI turns each
// item into an inbox task. Format: zunder_local::share::SharedItem.

import Foundation

enum ShareInbox {
    #if os(iOS)
    static let appGroup = "group.dev.rubeen.zunder"
    #else
    // macOS: the team ID as prefix needs no provisioning profile (Developer ID).
    static let appGroup = "H95J852PKP.dev.rubeen.zunder"
    #endif
    static let folder = "share-inbox"
    /// The app reads at most 1 MiB per file; long texts are cut well before.
    static let maxTextLength = 100_000
    /// The app's limit per file (zunder_local::share::MAX_FILE_BYTES).
    static let maxFileBytes = 1024 * 1024

    enum Failure: LocalizedError {
        case noContainer
        case tooLarge

        var errorDescription: String? {
            switch self {
            case .noContainer:
                return "Zunder konnte den gemeinsamen Ordner mit der App nicht öffnen."
            case .tooLarge:
                return "Der geteilte Text ist zu lang für eine Aufgabe."
            }
        }
    }

    private struct Item: Encodable {
        let version: Int
        let id: String
        let text: String?
        let url: String?
        // swiftlint:disable:next identifier_name
        let created_at_ms: Int64
    }

    static func save(text: String?, url: URL?) throws {
        guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup) else {
            throw Failure.noContainer
        }
        let dir = container.appendingPathComponent(folder, isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

        let id = UUID().uuidString.lowercased()
        let trimmed = text?.trimmingCharacters(in: .whitespacesAndNewlines)
        let item = Item(
            version: 1,
            id: id,
            text: trimmed.map { String($0.prefix(maxTextLength)) },
            url: url?.absoluteString,
            created_at_ms: Int64(Date().timeIntervalSince1970 * 1000)
        )
        let data = try JSONEncoder().encode(item)
        // Characters can take many bytes; the app would set a larger file aside.
        guard data.count <= maxFileBytes else { throw Failure.tooLarge }
        // Atomic: the app never sees a half-written file under the final name.
        try data.write(to: dir.appendingPathComponent("\(id).json"), options: .atomic)
    }
}
