mod cli;
mod recorder;
mod web;

fn main() -> std::io::Result<()> {
    if std::env::args().nth(1).map(|s| s == "--gc") == Some(true) {
        cli::gc()
    } else {
        web::start()
    }
}
