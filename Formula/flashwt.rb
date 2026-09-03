class Flashwt < Formula
  desc "Instant git worktrees with heavy directories already hydrated"
  homepage "https://github.com/__REPO__"
  version "__VERSION__"

  DOWNLOAD_BASE = "__DOWNLOAD_BASE__"

  on_macos do
    on_arm do
      url "#{DOWNLOAD_BASE}/v#{version}/flashwt-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "__SHA256_AARCH64_APPLE_DARWIN__"
    end
    on_intel do
      url "#{DOWNLOAD_BASE}/v#{version}/flashwt-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "__SHA256_X86_64_APPLE_DARWIN__"
    end
  end
  on_linux do
    url "#{DOWNLOAD_BASE}/v#{version}/flashwt-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
    sha256 "__SHA256_X86_64_UNKNOWN_LINUX_GNU__"
  end

  def install
    bin.install "flashwt"

    bash_output = Utils.safe_popen_read("#{bin}/flashwt", "completions", "bash")
    (bash_completion/"flashwt").write bash_output
    zsh_output = Utils.safe_popen_read("#{bin}/flashwt", "completions", "zsh")
    (zsh_completion/"_flashwt").write zsh_output
    fish_output = Utils.safe_popen_read("#{bin}/flashwt", "completions", "fish")
    (fish_completion/"flashwt.fish").write fish_output
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/flashwt --version")
  end
end

