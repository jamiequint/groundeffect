import SwiftUI
import Foundation

class MenuViewModel: ObservableObject {
    @Published var accounts: [AccountInfo] = []
    @Published var emailCount: Int = 0
    @Published var eventCount: Int = 0
    @Published var lastSyncText: String = "Never"
    @Published var recentItems: [RecentItem] = []

    init() {
        // Load initial data
        refresh()

        // Set up periodic refresh
        Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in
            self?.refresh()
        }
    }

    func refresh() {
        // TODO: Read from daemon or database
        // For now, show placeholder data
        accounts = []
        emailCount = 0
        eventCount = 0
        lastSyncText = "Never"
        recentItems = []
    }

    func addAccount() {
        // TODO: Launch OAuth flow using ASWebAuthenticationSession
        print("Add account tapped")
    }

    func syncAll() {
        // TODO: Trigger sync via daemon
        print("Sync all tapped")
    }

    func openStatusWindow() {
        // TODO: Open status window
        print("Open status window tapped")
    }

    func openPreferences() {
        // Open Settings window
        NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
    }
}

struct AccountInfo: Identifiable {
    let id: String
    let email: String
    let alias: String?
    let displayName: String
    let needsReauth: Bool
    let isSyncing: Bool

    var statusIcon: String {
        if needsReauth {
            return "exclamationmark.triangle.fill"
        } else if isSyncing {
            return "arrow.clockwise"
        } else {
            return "checkmark.circle.fill"
        }
    }

    var statusColor: Color {
        if needsReauth {
            return .orange
        } else if isSyncing {
            return .blue
        } else {
            return .green
        }
    }
}

struct RecentItem: Identifiable {
    let id: String
    let type: ItemType
    let title: String
    let accountAlias: String?

    var icon: String {
        switch type {
        case .email:
            return "envelope.fill"
        case .event:
            return "calendar"
        }
    }

    enum ItemType {
        case email
        case event
    }
}
