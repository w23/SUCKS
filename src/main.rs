#![allow(non_snake_case)]
use std::env;
//use env_logger;

mod socks;
mod exit;
mod ste;
mod ringbuf;

/*
#[derive(Debug)]
struct SimpleError {
    what: String
}

impl SimpleError {
    fn new<T: Into<String>>(t: T) -> SimpleError {
        SimpleError{ what: t.into() }
    }
}

impl std::fmt::Display for SimpleError {
    #[inline]
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.what.fmt(f)
    }
}

impl std::error::Error for SimpleError {}
*/

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let args: Vec<String> = env::args().collect();
    socks::main(&args[1], args.get(2).unwrap_or(&String::from("")))
}
