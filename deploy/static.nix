# SPDX-License-Identifier: GPL-3.0-or-later
# A statically linked (musl) build of one commit, runnable on any x86-64 Linux.
# Only committed files are built, so .env and room state never enter the store.
#   nix build --impure --file deploy/static.nix --argstr rev "$(git rev-parse HEAD)"
{
  rev,
  repo ? toString ./..,
  nixpkgs ? builtins.getFlake "nixpkgs",
}:
let
  pkgs = nixpkgs.legacyPackages.x86_64-linux.pkgsStatic;
  src = builtins.fetchGit {
    url = repo;
    inherit rev;
  };
in
pkgs.rustPlatform.buildRustPackage {
  pname = "discord-bot";
  version = "0.1.0";
  inherit src;
  cargoLock.lockFile = "${src}/Cargo.lock";
  # CI runs the tests; this is only the release build.
  doCheck = false;
}
