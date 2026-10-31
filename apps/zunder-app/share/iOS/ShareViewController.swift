// Share sheet on iOS: the system's compose sheet with the shared text (or the
// page title) as editable title; "Sichern" leaves the item for the app.

import Social
import UIKit

@objc(ShareViewController)
final class ShareViewController: SLComposeServiceViewController {
    private var sharedURL: URL?
    private var reading = true

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Zunder"
        placeholder = "Titel der Aufgabe"
        let items = extensionContext?.inputItems.compactMap { $0 as? NSExtensionItem } ?? []
        ShareReader.read(items) { [weak self] content in
            guard let self else { return }
            self.reading = false
            self.sharedURL = content.url
            if self.contentText.trimmed.isEmpty, let text = content.text {
                self.textView.text = text
            }
            self.validateContent()
        }
    }

    override func presentationAnimationDidFinish() {
        super.presentationAnimationDidFinish()
        navigationController?.navigationBar.topItem?.rightBarButtonItem?.title = "Sichern"
    }

    override func isContentValid() -> Bool {
        !reading && (!contentText.trimmed.isEmpty || sharedURL != nil)
    }

    override func didSelectPost() {
        do {
            try ShareInbox.save(text: contentText, url: sharedURL)
            extensionContext?.completeRequest(returningItems: [], completionHandler: nil)
        } catch {
            extensionContext?.cancelRequest(withError: error)
        }
    }

    override func configurationItems() -> [Any]! {
        guard let target = SLComposeSheetConfigurationItem() as SLComposeSheetConfigurationItem? else { return [] }
        target.title = "Ablage"
        target.value = "Inbox"
        return [target]
    }
}
