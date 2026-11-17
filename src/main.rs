mod filter;
mod har;
mod save;
mod tui;

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use filter::Filters;
use har::format_size;
use save::OutLayout;

/// Browse a HAR file, filter its responses and save them to a folder.
///
/// Opens an interactive TUI by default (flags pre-fill its filters).
/// Use --list or --yes to run without the TUI.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// The .har file
    file: PathBuf,

    /// Output folder
    #[arg(short, long, default_value = "dist")]
    out: PathBuf,

    /// Types or extensions, comma separated (image,script,css,font,json,html,video,audio,png,...)
    #[arg(short = 't', long = "type", value_delimiter = ',')]
    types: Vec<String>,

    /// Domains, comma separated (example.com also matches cdn.example.com)
    #[arg(short, long, value_delimiter = ',')]
    domain: Vec<String>,

    /// URL regex (case-insensitive) or plain text
    #[arg(short = 'm', long = "match")]
    pattern: Option<String>,

    /// Status codes, x = wildcard (200, 2xx, 2xx,304)
    #[arg(short, long)]
    status: Option<String>,

    /// Minimum size (500, 10k, 1.5m)
    #[arg(long, value_parser = parse_size_arg)]
    min_size: Option<usize>,

    /// How files are laid out in the output folder
    #[arg(long, value_enum, default_value_t = OutLayout::Tree)]
    layout: OutLayout,

    /// Also show entries whose body was not recorded in the HAR
    #[arg(long)]
    include_empty: bool,

    /// Print matching entries and exit (no TUI)
    #[arg(short, long)]
    list: bool,

    /// Save matching entries directly (no TUI)
    #[arg(short, long)]
    yes: bool,
}

fn parse_size_arg(s: &str) -> Result<usize, String> {
    har::parse_size(s).ok_or_else(|| format!("invalid size '{s}' (examples: 500, 10k, 1.5m)"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let entries = har::load(&cli.file)?;

    let mut filters = Filters::default();
    filters.types = cli.types.iter().map(|s| s.trim().to_lowercase()).collect();
    filters.domains = cli.domain.iter().map(|s| s.trim().to_lowercase()).collect();
    filters.status = cli.status.unwrap_or_default();
    filters.min_size = cli.min_size.unwrap_or(0);
    filters.include_empty = cli.include_empty;
    if let Some(p) = &cli.pattern {
        filters.set_query(p);
    }

    if cli.list || cli.yes {
        let matching: Vec<&har::Entry> = entries.iter().filter(|e| filters.matches(e)).collect();
        if cli.list {
            for e in &matching {
                println!("{:<4} {:<7} {:>9}  {}", e.status, e.category, format_size(e.size), e.display_url());
            }
            let total: usize = matching.iter().map(|e| e.size).sum();
            println!("\n{} of {} entries, {}", matching.len(), entries.len(), format_size(total));
        }
        if cli.yes {
            let r = save::save(&matching, &cli.out, cli.layout);
            println!("{}", tui::report_text(&r, &cli.out));
            for err in &r.errors {
                eprintln!("  error: {err}");
            }
        }
        return Ok(());
    }

    let name = cli.file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    tui::run(entries, filters, cli.out, cli.layout, name)
}
