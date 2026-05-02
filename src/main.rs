use clap::Parser;

mod audit;
mod cli;
mod hook;
mod jsonfmt;
mod project_docs;
mod project_init;
mod projects;
mod setup;
mod status;
mod statusline;
mod transcript;
mod usage;
mod vault;

fn main() {
    let exit = match cli::Cli::parse().run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            1
        }
    };
    std::process::exit(exit);
}
