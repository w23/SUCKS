#![allow(non_snake_case)]
use std::env;

mod log;
mod socks;
mod exit;
mod ste;
mod ringbuf;

static LOGGER: log::Logger = log::Logger;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    ::log::set_logger(&LOGGER)
        .map(|()| ::log::set_max_level(::log::LevelFilter::Trace)).unwrap();

    let args: Vec<String> = env::args().collect();
    socks::main(&args[1], args.get(2).unwrap_or(&String::from("")))
}
