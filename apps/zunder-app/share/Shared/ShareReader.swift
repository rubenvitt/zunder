// Reads what another app shares: the first web link and the text (selected
// text, a note, or the page title some apps send along).

import Foundation
import UniformTypeIdentifiers

struct SharedContent {
    var text: String?
    var url: URL?
}

enum ShareReader {
    private final class Collector {
        let lock = NSLock()
        var url: URL?
        var texts: [String] = []
    }

    /// Calls `completion` on the main queue once all attachments are read.
    static func read(_ items: [NSExtensionItem], completion: @escaping (SharedContent) -> Void) {
        let collector = Collector()
        let group = DispatchGroup()
        var title: String?

        for item in items {
            if title == nil, let text = item.attributedContentText?.string, !text.trimmed.isEmpty {
                title = text
            }
            if title == nil, let text = item.attributedTitle?.string, !text.trimmed.isEmpty {
                title = text
            }
            for provider in item.attachments ?? [] {
                if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier) {
                    group.enter()
                    provider.loadItem(forTypeIdentifier: UTType.url.identifier, options: nil) { value, _ in
                        defer { group.leave() }
                        guard let url = ShareReader.asURL(value), !url.isFileURL else { return }
                        collector.lock.lock()
                        if collector.url == nil { collector.url = url }
                        collector.lock.unlock()
                    }
                } else if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier) {
                    group.enter()
                    provider.loadItem(forTypeIdentifier: UTType.plainText.identifier, options: nil) { value, _ in
                        defer { group.leave() }
                        guard let text = ShareReader.asText(value), !text.trimmed.isEmpty else { return }
                        collector.lock.lock()
                        collector.texts.append(text)
                        collector.lock.unlock()
                    }
                }
            }
        }

        group.notify(queue: .main) {
            collector.lock.lock()
            let texts = collector.texts
            let url = collector.url
            collector.lock.unlock()
            var text = title
            // Selected text or a note beats the page title.
            if let shared = texts.first(where: { ShareReader.asURL($0) == nil }) {
                text = shared
            }
            // A link shared as text only (some apps do that).
            let link = url ?? texts.lazy.compactMap { ShareReader.asURL($0) }.first
            completion(SharedContent(text: text, url: link))
        }
    }

    private static func asURL(_ value: Any?) -> URL? {
        switch value {
        case let url as URL:
            return url
        case let data as Data:
            return URL(dataRepresentation: data, relativeTo: nil)
        case let string as String:
            let trimmed = string.trimmed
            guard !trimmed.contains(where: \.isWhitespace),
                  let url = URL(string: trimmed),
                  let scheme = url.scheme?.lowercased(),
                  scheme == "http" || scheme == "https"
            else { return nil }
            return url
        default:
            return nil
        }
    }

    private static func asText(_ value: Any?) -> String? {
        switch value {
        case let string as String:
            return string
        case let attributed as NSAttributedString:
            return attributed.string
        case let data as Data:
            return String(data: data, encoding: .utf8)
        default:
            return nil
        }
    }
}

extension String {
    var trimmed: String { trimmingCharacters(in: .whitespacesAndNewlines) }
}
