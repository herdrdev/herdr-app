# This repo is its own tap:
#
#   brew tap herdrdev/herdr-app https://github.com/herdrdev/herdr-app
#   brew install --cask herdr-app
#
# Unversioned on purpose: the DMG's checksum exists only after the tag's
# notarization, too late to land in this file. The DMG is signed and
# notarized, and the app updates itself, so Homebrew only installs it.
cask "herdr-app" do
  version :latest
  sha256 :no_check

  url "https://releases.herdr.dev/app/Herdr.dmg"
  name "Herdr"
  desc "Terminals and coding agents that keep running"
  homepage "https://herdr.dev/"

  depends_on arch: :arm64
  depends_on :macos

  app "Herdr.app"
  binary "#{appdir}/Herdr.app/Contents/MacOS/Herdr", target: "herdr-app"

  zap trash: [
    "~/.config/herdr-app",
    "~/Library/Caches/dev.herdr.app",
    "~/Library/Preferences/dev.herdr.app.plist",
    "~/Library/Saved Application State/dev.herdr.app.savedState",
  ]

  caveats <<~EOS
    Herdr needs herdr-core. If it is missing, Herdr offers to install it when it opens.
  EOS
end
