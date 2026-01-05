class Groundeffect < Formula
  desc "Hyper-fast, private email and calendar indexing for Claude Code"
  homepage "https://github.com/jamiequint/groundeffect"
  url "https://github.com/jamiequint/groundeffect/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "ade6cb147722b31a4eaff2e0c87bcbded3bcb2a3e3cc71f7fd40be75a4b86f1c"
  license "MIT"

  depends_on "rust" => :build

  def install
    system "cargo", "build", "--release"
    bin.install "target/release/groundeffect-daemon"
    bin.install "target/release/groundeffect-mcp"
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

      4. Add a Google account:
         groundeffect-daemon add-account

      Get OAuth credentials from: https://console.cloud.google.com/apis/credentials
      (Create OAuth 2.0 Client ID > Desktop app, enable Gmail & Calendar APIs)
    EOS
  end

  # Runs BEFORE binaries are removed
  def uninstall_preflight
    # Remove launchd agent if installed
    system "#{bin}/groundeffect-daemon", "setup", "--uninstall" rescue nil
  end

  def post_uninstall
    # Clean up data directories (but not ~/.secrets or ~/.claude.json)
    rm_rf Dir.home + "/.config/groundeffect"
    rm_rf Dir.home + "/.local/share/groundeffect"
  end

  test do
    assert_match "groundeffect-daemon", shell_output("#{bin}/groundeffect-daemon --help")
  end
end
