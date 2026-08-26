use clap::Parser;
use slideforge::cli::Cli;

fn main() {
    std::process::exit(Cli::parse().run());
}
