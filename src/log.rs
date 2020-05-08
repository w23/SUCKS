//use std::thread;
use std::cell::RefCell;
use log::{self, Record, Level, Metadata};

pub struct Logger;

thread_local!(static PREFIX: RefCell<String> = RefCell::new(String::new()));

pub struct Context {
    prevfix: String,
}

impl Context {
    pub fn new(prefix: String) -> Context {
        let prevfix =
        PREFIX.with(|p| {
            let mut prefix = prefix;
            std::mem::swap(&mut prefix, &mut p.borrow_mut());
            return prefix;
        });

        return Context{ prevfix };
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        PREFIX.with(|p| {
            std::mem::swap(&mut self.prevfix, &mut p.borrow_mut());
        });
    }
}

impl log::Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            PREFIX.with(|p| {
                println!("{}{} - {}", p.borrow(), record.level(), record.args());
            });
        }
    }

    fn flush(&self) {}
}
