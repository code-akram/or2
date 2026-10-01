# Homebrew formula for or2-pair, built from source. Not in a public tap yet: see docs/pairing.md.
# A stable `url` and `sha256` are added when the first release is tagged; until then use --HEAD.
class Or2Pair < Formula
  desc "Pair a phone with this host: one command, one QR scan (or2 Easy pair)"
  homepage "https://github.com/code-akram/or2"
  license "GPL-3.0-or-later"
  head "https://github.com/code-akram/or2.git", branch: "main"

  depends_on "rust" => :build

  def install
    # core/Cargo.lock pins every dependency. Only the host tool is built: the app library and its
    # Zig-built terminal engine are not needed here.
    system "cargo", "install", "--locked", "--root", prefix, "--path", "core/or2-pair"
  end

  test do
    assert_match "or2-pair #{version}", shell_output("#{bin}/or2-pair --version") if build.stable?
    assert_match "pair a phone with this host", shell_output("#{bin}/or2-pair --help")
    # Checks only: no listener, no files written.
    assert_match "Nothing was changed", shell_output("#{bin}/or2-pair --check")
  end
end
