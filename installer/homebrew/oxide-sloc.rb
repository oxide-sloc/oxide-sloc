# Homebrew formula for oxide-sloc
#
# To use with a private tap:
#   brew tap oxide-sloc/oxide-sloc https://github.com/oxide-sloc/homebrew-oxide-sloc
#   brew install oxide-sloc/oxide-sloc/oxide-sloc
#
# To build from source (requires Rust installed):
#   brew install --build-from-source oxide-sloc
#
# The sha256 checksum below must be updated on each release.
# Run: curl -L <url> | sha256sum

class OxideSloc < Formula
  desc "IEEE 1045-1992 SLOC analysis, test detection, and code metrics workbench"
  homepage "https://github.com/oxide-sloc/oxide-sloc"
  url "https://github.com/oxide-sloc/oxide-sloc/archive/refs/tags/v1.6.28.tar.gz"
  sha256 "18ac5c1b74e034eec79b9e67119c8148fe2ed87152efbe6cf0fa81779add1499"
  license "AGPL-3.0-or-later"
  head "https://github.com/oxide-sloc/oxide-sloc.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args(path: "crates/sloc-cli")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/oxide-sloc --version")
    (testpath/"hello.rs").write "fn main() { println!(\"hello\"); }\n"
    output = shell_output("#{bin}/oxide-sloc analyze #{testpath} --plain")
    assert_match "Rust", output
  end
end
