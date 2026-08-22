# Template for the wt Homebrew formula. The placeholder tokens here are
# filled in by scripts/gen-formula.sh at release time; the generated
# file is attached to each GitHub release.
class Wt < Formula
  desc "Instant git worktrees with heavy directories already hydrated"
  homepage "https://github.com/__REPO__"
  version "__VERSION__"

  # Per-tag directory of a GitHub release; smoke tests swap this for a
  # file:// path laid out the same way.
  DOWNLOAD_BASE = "__DOWNLOAD_BASE__"

  on_macos do
    on_arm do
      url "#{DOWNLOAD_BASE}/v#{version}/wt-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "__SHA256_AARCH64_APPLE_DARWIN__"
    end
    on_intel do
      url "#{DOWNLOAD_BASE}/v#{version}/wt-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "__SHA256_X86_64_APPLE_DARWIN__"
    end
  end
  on_linux do
    url "#{DOWNLOAD_BASE}/v#{version}/wt-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "__SHA256_X86_64_UNKNOWN_LINUX_GNU__"
  end

  def install
    bin.install "wt"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/wt --version")
  end
end
