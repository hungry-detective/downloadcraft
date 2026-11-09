//! A complete command-line downloader in ~60 lines.
//!
//! ```text
//! cargo run --example get -- https://example.com/big.iso [big.iso] [connections]
//! ```
//! Ctrl+C stops it; running the same command again resumes.

use downloadcraft_engine::{Config, Control, Downloader, Job, Progress};
use std::io::Write;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let Some(url) = args.next() else {
        eprintln!("usage: get <url> [file] [connections]");
        std::process::exit(2);
    };
    let file = args.next().unwrap_or_else(|| {
        let name = url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
        if name.is_empty() { "download.bin".into() } else { name.into() }
    });
    let connections = args.next().and_then(|c| c.parse().ok()).unwrap_or(8);

    let progress = Arc::new(Progress::default());
    let control = Arc::new(Control::default());
    let c = control.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        c.cancel();
    });
    let p = progress.clone();
    let ticker = tokio::spawn(async move {
        let (mut last, mut at) = (p.downloaded(), Instant::now());
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let now = p.downloaded();
            let speed = (now - last.min(now)) as f64 / at.elapsed().as_secs_f64() / 1_048_576.0;
            (last, at) = (now, Instant::now());
            let size = p.total().map(|t| format!("{:.1} MiB", t as f64 / 1_048_576.0)).unwrap_or("?".into());
            print!(
                "\r{:.1} MiB of {size}  {speed:.2} MiB/s  {} connection(s)   ",
                now as f64 / 1_048_576.0,
                p.connections()
            );
            let _ = std::io::stdout().flush();
        }
    });

    let t0 = Instant::now();
    let downloader = Downloader::new(reqwest::Client::new())
        .config(Config { connections, ..Config::default() })
        .on_event(Arc::new(move |e| {
            // DCE_TRACE=1 prints every event with the time since the start.
            if std::env::var_os("DCE_TRACE").is_some() {
                eprintln!("{:7.3} {e}", t0.elapsed().as_secs_f64());
            } else if !matches!(e, downloadcraft_engine::Event::Connecting { .. }) {
                eprintln!("\n{e}");
            }
        }));
    let result = downloader.download(Job::new(url, &file), progress, control).await;
    ticker.abort();
    match result {
        Ok(done) => println!(
            "\nsaved {} ({} bytes, up to {} connections{})",
            done.path.display(),
            done.size,
            done.peak_connections,
            if done.resumed > 0 { format!(", {} bytes resumed", done.resumed) } else { String::new() }
        ),
        Err(e) => {
            eprintln!("\n{e}");
            std::process::exit(1);
        }
    }
}
