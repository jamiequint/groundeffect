import SwiftUI

struct SettingsView: View {
    var body: some View {
        TabView {
            GeneralSettingsView()
                .tabItem {
                    Label("General", systemImage: "gear")
                }

            AccountsSettingsView()
                .tabItem {
                    Label("Accounts", systemImage: "person.2")
                }

            SyncSettingsView()
                .tabItem {
                    Label("Sync", systemImage: "arrow.clockwise")
                }

            AdvancedSettingsView()
                .tabItem {
                    Label("Advanced", systemImage: "slider.horizontal.3")
                }
        }
        .frame(width: 500, height: 400)
    }
}

struct GeneralSettingsView: View {
    @AppStorage("launchAtLogin") private var launchAtLogin = false
    @AppStorage("showMenuBarIcon") private var showMenuBarIcon = true
    @AppStorage("showRecentItems") private var showRecentItems = 5

    var body: some View {
        Form {
            Toggle("Launch at login", isOn: $launchAtLogin)
            Toggle("Show menu bar icon", isOn: $showMenuBarIcon)

            Stepper(value: $showRecentItems, in: 0...10) {
                Text("Recent items to show: \(showRecentItems)")
            }
        }
        .padding()
    }
}

struct AccountsSettingsView: View {
    @State private var accounts: [AccountInfo] = []

    var body: some View {
        VStack {
            if accounts.isEmpty {
                ContentUnavailableView {
                    Label("No Accounts", systemImage: "person.crop.circle.badge.plus")
                } description: {
                    Text("Add a Gmail account to get started")
                } actions: {
                    Button("Add Account") {
                        addAccount()
                    }
                    .buttonStyle(.borderedProminent)
                }
            } else {
                List {
                    ForEach(accounts) { account in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(account.email)
                                    .font(.headline)
                                if let alias = account.alias {
                                    Text("Alias: \(alias)")
                                        .font(.caption)
                                        .foregroundColor(.secondary)
                                }
                            }

                            Spacer()

                            if account.needsReauth {
                                Button("Re-authenticate") {
                                    reauthenticate(account)
                                }
                                .buttonStyle(.bordered)
                            }

                            Button(role: .destructive) {
                                removeAccount(account)
                            } label: {
                                Image(systemName: "trash")
                            }
                            .buttonStyle(.borderless)
                        }
                    }
                }

                HStack {
                    Spacer()
                    Button("Add Account") {
                        addAccount()
                    }
                }
                .padding()
            }
        }
    }

    private func addAccount() {
        // TODO: Launch OAuth flow
        print("Add account")
    }

    private func reauthenticate(_ account: AccountInfo) {
        // TODO: Re-authenticate account
        print("Re-authenticate \(account.email)")
    }

    private func removeAccount(_ account: AccountInfo) {
        // TODO: Remove account
        print("Remove \(account.email)")
    }
}

struct SyncSettingsView: View {
    @AppStorage("emailIdleEnabled") private var emailIdleEnabled = true
    @AppStorage("emailPollInterval") private var emailPollInterval = 300
    @AppStorage("calendarPollInterval") private var calendarPollInterval = 300
    @AppStorage("maxConcurrentFetches") private var maxConcurrentFetches = 10

    var body: some View {
        Form {
            Toggle("Use IMAP IDLE for real-time push", isOn: $emailIdleEnabled)

            Picker("Email poll interval", selection: $emailPollInterval) {
                Text("1 minute").tag(60)
                Text("5 minutes").tag(300)
                Text("15 minutes").tag(900)
                Text("30 minutes").tag(1800)
            }

            Picker("Calendar poll interval", selection: $calendarPollInterval) {
                Text("1 minute").tag(60)
                Text("5 minutes").tag(300)
                Text("15 minutes").tag(900)
                Text("30 minutes").tag(1800)
            }

            Stepper(value: $maxConcurrentFetches, in: 1...20) {
                Text("Max concurrent fetches: \(maxConcurrentFetches)")
            }
        }
        .padding()
    }
}

struct AdvancedSettingsView: View {
    @AppStorage("logLevel") private var logLevel = "info"
    @AppStorage("useMetal") private var useMetal = true

    var body: some View {
        Form {
            Picker("Log level", selection: $logLevel) {
                Text("Debug").tag("debug")
                Text("Info").tag("info")
                Text("Warning").tag("warn")
                Text("Error").tag("error")
            }

            Toggle("Use Metal GPU acceleration", isOn: $useMetal)

            Section {
                HStack {
                    Text("Data directory:")
                    Text("~/.local/share/groundeffect")
                        .foregroundColor(.secondary)
                    Spacer()
                    Button("Open") {
                        openDataDirectory()
                    }
                }

                HStack {
                    Text("Config file:")
                    Text("~/.config/groundeffect/config.toml")
                        .foregroundColor(.secondary)
                    Spacer()
                    Button("Edit") {
                        openConfigFile()
                    }
                }
            }
        }
        .padding()
    }

    private func openDataDirectory() {
        let path = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".local/share/groundeffect")
        NSWorkspace.shared.open(path)
    }

    private func openConfigFile() {
        let path = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/groundeffect/config.toml")
        NSWorkspace.shared.open(path)
    }
}

#Preview {
    SettingsView()
}
