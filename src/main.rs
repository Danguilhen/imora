//! imora — a lightweight, elegant media gallery.

mod app;
mod media;
mod thumbs;
mod video;

use std::path::PathBuf;

use eframe::egui;

const HELP: &str = "\
imora — a lightweight media gallery

USAGE:
    imora [OPTIONS] [FOLDER]

ARGS:
    <FOLDER>    Folder to open at launch

OPTIONS:
    -d, --decorations   Show window decorations (minimize/maximize/close).
                        Default: none — imora follows systems that run with
                        window decorations disabled.
    -f, --fullscreen    Start in fullscreen
    -h, --help          Print this help
    -v, --version       Print version
";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Best-effort; if FFmpeg is missing, videos simply report an error in-app.
    let _ = ffmpeg_next::init();

    let mut folder: Option<PathBuf> = None;
    let mut fullscreen = false;
    let mut decorations = false;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                return Ok(());
            }
            "-v" | "--version" => {
                println!("imora {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-f" | "--fullscreen" => fullscreen = true,
            "-d" | "--decorations" => decorations = true,
            arg if arg.starts_with('-') => {
                eprintln!("unknown option: {arg}");
                eprint!("{HELP}");
                std::process::exit(2);
            }
            arg => {
                if folder.is_some() {
                    eprintln!("expected a single folder argument");
                    std::process::exit(2);
                }
                folder = Some(PathBuf::from(arg));
            }
        }
        i += 1;
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("imora")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([640.0, 420.0])
            .with_decorations(decorations),
        renderer: eframe::Renderer::Glow,
        centered: true,
        ..Default::default()
    };

    eframe::run_native(
        "imora",
        options,
        Box::new(move |cc| Ok(Box::new(app::ImoraApp::new(cc, folder, fullscreen)))),
    )
    .map_err(|e| e.into())
}
