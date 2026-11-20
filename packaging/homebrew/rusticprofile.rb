# THIS IS A TEMPLATE. The VERSION and SHA256 sentinels are filled in by `just brew-publish`
# (through scripts/render_packaging.py) when pushing to the tap. Do not replace them here;
# `just brew-check` fails if either sentinel is gone.
# Do not write at-sign sentinel names in comments: render substitutes them and
# brew audit --strict rejects any line longer than 118 characters.
class Rusticprofile < Formula
  desc "Local, per-machine scheduler and orchestrator for rustic backups"
  homepage "https://github.com/l1a/rusticprofile"
  url "https://github.com/l1a/rusticprofile/archive/refs/tags/v@VERSION@.tar.gz"
  sha256 "@SHA256@"
  license "GPL-3.0-or-later"
  head "https://github.com/l1a/rusticprofile.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args

    man1.install "docs/rusticprofile.1"

    generate_completions_from_executable(
      bin/"rusticprofile",
      shell_parameter_format: "--completions=",
    )
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/rusticprofile --version")
  end
end
