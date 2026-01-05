class Groundeffect < Formula
  desc "Hyper-fast, private email and calendar indexing for Claude Code"
  homepage "https://github.com/jamiequint/groundeffect"
  version "0.1.4"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/jamiequint/groundeffect/releases/download/v0.1.4/groundeffect-0.1.4-darwin-arm64.tar.gz"
      sha256 "b3719aafb3db46feb56f4b419af8b749ebb838baca619f111959c1c0a65aa9f9"
    end
  end

  def install
    bin.install "groundeffect-daemon"
    bin.install "groundeffect-mcp"
  end

  def post_install
    # Restart daemon if it's running (e.g., after upgrade)
    system "launchctl", "kickstart", "-k", "gui/#{Process.uid}/com.groundeffect.daemon" rescue nil
  end

  def caveats
    <<~EOS
      To complete setup:

      1. Create ~/.secrets with your Google OAuth credentials:
         export GROUNDEFFECT_GOOGLE_CLIENT_ID="your-client-id"
         export GROUNDEFFECT_GOOGLE_CLIENT_SECRET="your-client-secret"

         Then add to ~/.zshrc or ~/.bashrc:
         source ~/.secrets

      2. Add MCP config to ~/.claude.json:
         {
           "mcpServers": {
             "groundeffect": {
               "type": "stdio",
               "command": "groundeffect-mcp",
               "env": {
                 "GROUNDEFFECT_GOOGLE_CLIENT_ID": "${GROUNDEFFECT_GOOGLE_CLIENT_ID}",
                 "GROUNDEFFECT_GOOGLE_CLIENT_SECRET": "${GROUNDEFFECT_GOOGLE_CLIENT_SECRET}"
               }
             }
           }
         }

      3. Run the setup wizard:
         groundeffect-daemon setup --install

         Settings can be changed later with: groundeffect-daemon configure
         Note: "Max concurrent fetches" = IMAP connections (Gmail limit: 15)

      4. Add a Google account by asking Claude Code:
         "Add my Gmail account to groundeffect"

      Get OAuth credentials from: https://console.cloud.google.com/apis/credentials
      (Create OAuth 2.0 Client ID > Desktop app, enable Gmail & Calendar APIs)
    EOS
  end

  def uninstall_preflight
    system "#{bin}/groundeffect-daemon", "setup", "--uninstall" rescue nil
  end

  def post_uninstall
    rm_rf Dir.home + "/.config/groundeffect"
    rm_rf Dir.home + "/.local/share/groundeffect"
  end

  test do
    assert_match "groundeffect-daemon", shell_output("#{bin}/groundeffect-daemon --help")
  end
end
