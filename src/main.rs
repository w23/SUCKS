#![allow(non_snake_case)]
use {
    std::env,
    simple_error::SimpleError,
};

mod log;
mod socks;
mod exit;
mod ste;
mod ringbuf;

const DEFAULT_SOCKS_ADDRESS: &'static str = "localhost:8001";
const DEFAULT_EXIT_ADDRESS: &'static str = "localhost:16002";

static LOGGER: log::Logger = log::Logger;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    ::log::set_logger(&LOGGER)
        .map(|()| ::log::set_max_level(::log::LevelFilter::Trace)).unwrap();

    let args: Vec<String> = env::args().collect();
    let args: Vec<&str> = args.iter().map(|s| &**s).collect();

    let mode = args.get(1).unwrap_or(&"");

    match mode {
        &"socks" => {
            let listen = args.get(2).unwrap_or(&DEFAULT_SOCKS_ADDRESS);
            socks::main(&listen, args.get(3).unwrap_or(&""))
        },
        &"exit" => {
            let listen = args.get(2).unwrap_or(&DEFAULT_EXIT_ADDRESS);
            exit::main(&listen)
        },
        _ => Err(Box::new(SimpleError::new(format!("Usage: {} <mode:socks|exit> <listen:port> <(socks only)exit_addr>", args[0]))))
    }
}
