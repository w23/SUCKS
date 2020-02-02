#![allow(non_snake_case)]
mod socks;
mod exit;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = clap::App::new("SUCKS")
        .setting(clap::AppSettings::SubcommandRequiredElseHelp)
        .subcommand(clap::SubCommand::with_name("exit")
            .about("Run in exit-node mode, accepting connections from SOCKSv5 server")
            .arg(clap::Arg::with_name("listen").help("Listen on address in host:port form").required(true)))
        .subcommand(clap::SubCommand::with_name("socks")
            .about("Run in SOCKSv5 server mode")
            .arg(clap::Arg::with_name("exit").help("Exit node address in host:port form").required(true))
            .arg(clap::Arg::with_name("listen").help("Listen on address in host:port form").required(true)))
        .get_matches();

    if let Some(cmd) = args.subcommand_matches("socks") {
        let exit = cmd.value_of("exit").unwrap();
        let listen = cmd.value_of("listen").unwrap();
        socks::main(listen, exit).await?
    }

    if let Some(cmd) = args.subcommand_matches("exit") {
        let listen = cmd.value_of("listen").unwrap();
        exit::main(listen).await?
    }

    let err : Box<dyn std::error::Error> = Box::new(SimpleError::new("Not implemented"));
    Err(err)
}
