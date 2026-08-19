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

        # --- Audio output (cpal) ---
        alsa-lib

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

        # --- Stream URL resolution (YouTube etc., see media.rs::resolve_stream) ---
        yt-dlp
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
              pkgs.alsa-lib
            ];

            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";

            shellHook = ''
              export RUST_BACKTRACE=1
              echo "imora dev shell — run 'cargo run --release -- <folder>'"
            '';
          };
        });

      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = let
            runtimeLibs = with pkgs; lib.makeLibraryPath [
              ffmpeg_8
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
              fontconfig
              freetype
            ];
          in
          pkgs.rustPlatform.buildRustPackage {
            pname = "imora";
            version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = with pkgs; [
              pkg-config
              clang
              copyDesktopItems
              makeWrapper
            ];

            buildInputs = with pkgs; [
              ffmpeg_8
              alsa-lib
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
              fontconfig
              freetype
            ];

            LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";

            # winit/glutin dlopen libwayland/libGL/libEGL at runtime instead of
            # linking them, so a plain buildRustPackage RUNPATH (which only
            # covers directly-linked libs) leaves them unfindable. Add every
            # runtime lib dir to the RUNPATH explicitly.
            postFixup = ''
              patchelf --add-rpath "${runtimeLibs}" "$out/bin/imora"
              # imora shells out to yt-dlp by name for streaming-site URLs
              # (media.rs::resolve_stream); put it on the binary's PATH.
              wrapProgram "$out/bin/imora" \
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.yt-dlp ]}
            '';

            desktopItems = [
              (pkgs.makeDesktopItem {
                name = "imora";
                exec = "imora";
                desktopName = "imora";
                genericName = "Media gallery";
                categories = [ "Graphics" "Viewer" ];
                startupNotify = false;
                # Everything classify() accepts in media.rs. Opening a file
                # positions the gallery at that file (see ImoraApp::new).
                mimeTypes = [
                  "image/jpeg"
                  "image/png"
                  "image/gif"
                  "image/webp"
                  "image/bmp"
                  "image/tiff"
                  "image/vnd.microsoft.icon"
                  "image/x-qoi"
                  "image/x-portable-anymap"
                  "image/x-dds"
                  "image/x-tga"
                  "image/x-exr"
                  "video/mp4"
                  "video/x-matroska"
                  "video/webm"
                  "video/quicktime"
                  "video/x-msvideo"
                  "video/mpeg"
                  "video/ogg"
                  "video/x-ms-wmv"
                  "video/x-flv"
                  "video/3gpp"
                  "video/mp2t"
                ];
              })
            ];

            meta = with pkgs.lib; {
              description = "A lightweight, elegant media gallery";
              homepage = "https://github.com/Danguilhen/imora";
              license = with licenses; [ mit asl20 ];
              mainProgram = "imora";
              platforms = [ "x86_64-linux" "aarch64-linux" ];
            };
          };
        });
    };
}
