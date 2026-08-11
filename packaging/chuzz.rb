# The Homebrew cask, committed here as the source of truth and copied into
# pathscale/homebrew-tap as Casks/chuzz.rb.
#
# It carries no version and no checksum, so it is copied over once and then
# never touched again: a release changes the bytes behind a fixed URL rather
# than this file. Keeping it in the app's own repository means the two are
# reviewed together, which is not true of a cask that only exists in the tap.
cask "chuzz" do
  # The download URL carries no version on purpose: each release overwrites the
  # previous tarball, so the CDN holds one file rather than a growing pile. That
  # makes `version :latest` the only honest value here, and Homebrew requires
  # `sha256 :no_check` alongside it, because the bytes behind a fixed URL change.
  #
  # The consequence worth knowing: `brew upgrade` can never detect a new
  # release, and unlike AgencyZero there is no in-app updater to compensate yet.
  # Until there is, moving an installed copy forward means
  # `brew reinstall --cask chuzz`.
  version :latest
  sha256 :no_check

  url "https://24x.ai/chuzz/Chuzz.app.tar.gz"
  name "Chuzz"
  desc "Pure Rust web browser built on the Pathscale Blitz engine"
  homepage "https://github.com/pathscale/chuzz"

  # arm64 only, deliberately. An Intel Mac would install this happily and then
  # fail to launch, so refuse at install time where the message is legible.
  depends_on arch: :arm64
  depends_on macos: :big_sur

  app "Chuzz.app"

  # The bundle is ad-hoc signed, not notarized, so Gatekeeper rejects it and
  # Homebrew quarantines every cask artifact since 5.1 removed --no-quarantine.
  # Without this the app dies on first launch claiming to be damaged.
  #
  # Delete this block once the bundle is notarized: leaving it in would strip a
  # Gatekeeper check that users should get.
  postflight do
    system_command "/usr/bin/xattr",
                   args: ["-dr", "com.apple.quarantine", "#{appdir}/Chuzz.app"]
  end

  # Chuzz keeps no profile on disk yet: history and cookies live in memory for
  # the life of the process. These are the paths macOS creates on its own.
  zap trash: [
    "~/Library/Caches/com.pathscale.chuzz",
    "~/Library/Saved Application State/com.pathscale.chuzz.savedState",
  ]
end
