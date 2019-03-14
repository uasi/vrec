mod cli;
mod recorder;
mod web;

fn main() -> std::io::Result<()> {
    if std::env::args().skip(1).next().map(|s| s == "--gc") == Some(true) {
        cli::gc()
    } else {
        web::start()
    }
}
