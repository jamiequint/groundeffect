import SwiftUI

struct MenuView: View {
    @StateObject private var viewModel = MenuViewModel()

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Header
            HStack {
                Image(systemName: "bolt.fill")
                    .foregroundColor(.yellow)
                Text("GroundEffect")
                    .font(.headline)
                Spacer()
            }
            .padding()
            .background(Color(NSColor.controlBackgroundColor))

            Divider()

            // Accounts section
            VStack(alignment: .leading, spacing: 4) {
                Text("Accounts:")
                    .font(.subheadline)
                    .foregroundColor(.secondary)
                    .padding(.horizontal)
                    .padding(.top, 8)

                if viewModel.accounts.isEmpty {
                    Text("No accounts configured")
                        .font(.caption)
                        .foregroundColor(.secondary)
                        .padding(.horizontal)
                        .padding(.vertical, 4)
                } else {
                    ForEach(viewModel.accounts) { account in
                        AccountRow(account: account)
                    }
                }
            }

            Divider()
                .padding(.vertical, 8)

            // Stats section
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Image(systemName: "envelope.fill")
                    Text("\(viewModel.emailCount) emails")
                    if viewModel.accounts.count > 1 {
                        Text("(\(viewModel.accounts.count) accounts)")
                            .foregroundColor(.secondary)
                    }
                }
                .font(.caption)
                .padding(.horizontal)

                HStack {
                    Image(systemName: "calendar")
                    Text("\(viewModel.eventCount) events")
                }
                .font(.caption)
                .padding(.horizontal)

                HStack {
                    Image(systemName: "clock")
                    Text("Last sync: \(viewModel.lastSyncText)")
                }
                .font(.caption)
                .foregroundColor(.secondary)
                .padding(.horizontal)
            }

            Divider()
                .padding(.vertical, 8)

            // Recent items section
            if !viewModel.recentItems.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Recent:")
                        .font(.subheadline)
                        .foregroundColor(.secondary)
                        .padding(.horizontal)

                    ForEach(viewModel.recentItems) { item in
                        RecentItemRow(item: item)
                    }
                }

                Divider()
                    .padding(.vertical, 8)
            }

            // Actions
            VStack(spacing: 0) {
                MenuButton(title: "Add Account...", icon: "plus.circle") {
                    viewModel.addAccount()
                }

                MenuButton(title: "Sync All Now", icon: "arrow.clockwise") {
                    viewModel.syncAll()
                }

                MenuButton(title: "Open Status Window", icon: "chart.bar") {
                    viewModel.openStatusWindow()
                }

                MenuButton(title: "Preferences...", icon: "gear") {
                    viewModel.openPreferences()
                }
            }

            Divider()

            MenuButton(title: "Quit GroundEffect", icon: "power") {
                NSApplication.shared.terminate(nil)
            }
            .padding(.bottom, 8)
        }
        .frame(width: 300)
    }
}

struct AccountRow: View {
    let account: AccountInfo

    var body: some View {
        HStack {
            Image(systemName: account.statusIcon)
                .foregroundColor(account.statusColor)
                .frame(width: 16)

            VStack(alignment: .leading, spacing: 2) {
                Text(account.displayName)
                    .font(.caption)
                if let alias = account.alias {
                    Text("(\(alias))")
                        .font(.caption2)
                        .foregroundColor(.secondary)
                }
            }

            Spacer()

            if account.needsReauth {
                Text("needs auth")
                    .font(.caption2)
                    .foregroundColor(.orange)
            }
        }
        .padding(.horizontal)
        .padding(.vertical, 4)
        .contentShape(Rectangle())
        .onTapGesture {
            // TODO: Show account details
        }
    }
}

struct RecentItemRow: View {
    let item: RecentItem

    var body: some View {
        HStack {
            Image(systemName: item.icon)
                .frame(width: 16)
                .foregroundColor(.secondary)

            if let accountAlias = item.accountAlias {
                Text("[\(accountAlias)]")
                    .font(.caption2)
                    .foregroundColor(.blue)
            }

            Text(item.title)
                .font(.caption)
                .lineLimit(1)

            Spacer()
        }
        .padding(.horizontal)
        .padding(.vertical, 2)
        .contentShape(Rectangle())
        .onTapGesture {
            // TODO: Open item
        }
    }
}

struct MenuButton: View {
    let title: String
    let icon: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack {
                Image(systemName: icon)
                    .frame(width: 20)
                Text(title)
                Spacer()
            }
            .padding(.horizontal)
            .padding(.vertical, 6)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .onHover { hovering in
            // Highlight on hover would be handled by custom hover effect
        }
    }
}

#Preview {
    MenuView()
}
