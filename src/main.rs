#![allow(non_snake_case)]
use std::env;

mod socks;
mod exit;
mod ste;
mod ringbuf;
mod log;

static LOGGER: log::Logger = log::Logger;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    ::log::set_logger(&LOGGER)
        .map(|()| ::log::set_max_level(::log::LevelFilter::Trace)).unwrap();

    let _logctx = log::Context::new("QEQ".to_string());

    let args: Vec<String> = env::args().collect();
    socks::main(&args[1], args.get(2).unwrap_or(&String::from("")))
}
