use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

mod screencast;

#[derive(Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    scenarios: Scenarios,
}

#[derive(Clone, ValueEnum)]
enum ScreenCastMode {
    File,
    Live,
}

#[derive(Subcommand)]
enum Scenarios {
    ScreenCast {
        #[arg(long)]
        mode: ScreenCastMode,
        #[arg(long, required_if_eq("mode", "file"))]
        file: Option<PathBuf>,
    },
    ScreenCapture {},
    AudioCast {},
    AudioCapture {},
}

fn main() {
    let cli = Cli::parse();

    match cli.scenarios {
        Scenarios::ScreenCast { file, .. } => {
            screencast::run(file);
        }
        _ => todo!(),
    }
}
