# Convenience wrapper so `nix-shell shell.nix` provides exactly the same
# environment as `nix develop` (single source of truth: flake.nix).
{ pkgs ? import (builtins.getFlake (toString ./.)).inputs.nixpkgs { } }:

let
  flake = builtins.getFlake (toString ./.);
in
flake.devShells.${pkgs.system}.default
