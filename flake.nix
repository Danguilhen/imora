{
  description = "imora — a lightweight, elegant media gallery";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);

      shellPkgs = pkgs: with pkgs; [
        # --- Rust toolchain ---
        cargo
        rustc
        rustfmt
        clippy
        rust-analyzer

        # --- Build essentials ---
        pkg-config
        gcc
        gnumake

        # --- FFmpeg (video decoding via ffmpeg-next) ---
        # ffmpeg_8 (8.0.x) matches ffmpeg-sys-next ^8.0 (ffmpeg-next 8.0.0)
        ffmpeg_8
        # bindgen (used by ffmpeg-sys-next) needs libclang
        clang
        libclang.lib

        # --- Windowing / OpenGL for eframe (glow renderer) ---
        libGL
        libxkbcommon
        wayland
        libxcb
        libx11
        libxrandr
        libxi
        libxcursor
        libxext
        libxrender

        # --- Fonts / text ---
        fontconfig
        freetype

        # --- Misc helpers used at runtime ---
        xdg-utils
      ];
    in
    {
      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            packages = shellPkgs pkgs;

            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
              pkgs.libGL
              pkgs.libxkbcommon
              pkgs.wayland
              pkgs.libxcb
              pkgs.libx11
              pkgs.libxrandr
              pkgs.libxi
              pkgs.libxcursor
              pkgs.libxext
              pkgs.libxrender
              pkgs.fontconfig
              pkgs.freetype
              pkgs.ffmpeg_8
            ];

            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";

            shellHook = ''
              export RUST_BACKTRACE=1
              echo "imora dev shell — run 'cargo run --release -- <folder>'"
            '';
          };
        });
    };
}
