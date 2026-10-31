// Share menu on macOS: a small sheet with the shared text (or the page title)
// as editable title and the link below; "In die Inbox" leaves the item for the
// app, which picks it up within a few seconds while it runs.

import Cocoa

@objc(ShareViewController)
final class ShareViewController: NSViewController {
    private let titleField = NSTextField()
    private let linkLabel = NSTextField(labelWithString: "")
    private let saveButton = NSButton(title: "In die Inbox", target: nil, action: nil)
    private let cancelButton = NSButton(title: "Abbrechen", target: nil, action: nil)
    private var sharedURL: URL?

    override func loadView() {
        let heading = NSTextField(labelWithString: "Neue Aufgabe in Zunder")
        heading.font = .boldSystemFont(ofSize: NSFont.systemFontSize)

        titleField.placeholderString = "Titel der Aufgabe"
        titleField.lineBreakMode = .byWordWrapping
        titleField.usesSingleLineMode = false
        titleField.cell?.wraps = true
        titleField.cell?.isScrollable = false
        titleField.maximumNumberOfLines = 4
        titleField.target = self
        titleField.action = #selector(save)
        titleField.delegate = self

        linkLabel.textColor = .secondaryLabelColor
        linkLabel.lineBreakMode = .byTruncatingMiddle
        linkLabel.isHidden = true

        saveButton.target = self
        saveButton.action = #selector(save)
        saveButton.keyEquivalent = "\r"
        saveButton.isEnabled = false
        cancelButton.target = self
        cancelButton.action = #selector(cancel)
        cancelButton.keyEquivalent = "\u{1b}"

        let buttons = NSStackView(views: [NSView(), cancelButton, saveButton])
        buttons.orientation = .horizontal

        let stack = NSStackView(views: [heading, titleField, linkLabel, buttons])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.edgeInsets = NSEdgeInsets(top: 16, left: 16, bottom: 16, right: 16)
        stack.translatesAutoresizingMaskIntoConstraints = false

        let root = NSView(frame: NSRect(x: 0, y: 0, width: 420, height: 160))
        root.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            stack.topAnchor.constraint(equalTo: root.topAnchor),
            stack.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            root.widthAnchor.constraint(equalToConstant: 420),
            titleField.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -32),
            linkLabel.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -32),
            buttons.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -32),
        ])
        view = root
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        let items = extensionContext?.inputItems.compactMap { $0 as? NSExtensionItem } ?? []
        ShareReader.read(items) { [weak self] content in
            guard let self else { return }
            self.sharedURL = content.url
            if self.titleField.stringValue.trimmed.isEmpty, let text = content.text {
                self.titleField.stringValue = text
            }
            if let url = content.url {
                self.linkLabel.stringValue = url.absoluteString
                self.linkLabel.isHidden = false
            }
            self.updateSaveButton()
        }
    }

    private func updateSaveButton() {
        saveButton.isEnabled = !titleField.stringValue.trimmed.isEmpty || sharedURL != nil
    }

    override func viewDidAppear() {
        super.viewDidAppear()
        view.window?.makeFirstResponder(titleField)
    }

    @objc private func save() {
        guard saveButton.isEnabled else { return }
        do {
            try ShareInbox.save(text: titleField.stringValue, url: sharedURL)
            extensionContext?.completeRequest(returningItems: nil, completionHandler: nil)
        } catch {
            NSAlert(error: error).runModal()
            extensionContext?.cancelRequest(withError: error)
        }
    }

    @objc private func cancel() {
        extensionContext?.cancelRequest(withError: NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError))
    }
}

extension ShareViewController: NSTextFieldDelegate {
    func controlTextDidChange(_ obj: Notification) {
        updateSaveButton()
    }
}
