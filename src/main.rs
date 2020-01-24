#![allow(non_snake_case)]
use futures::try_join;
use tokio::net::{TcpStream, TcpListener};
use std::net::IpAddr;
use std::io::{Read, Cursor, Seek};
use byteorder::{NetworkEndian, ReadBytesExt};

struct Connect {
    ver: u8,
    methods: Vec<u8>
}

async fn readConnect(socket: &mut tokio::net::TcpStream) -> Option<Connect> {
    let mut buf = [0; 258];
    let n = match tokio::io::AsyncReadExt::read(socket, &mut buf).await {
        Ok(n) if n < 3 => {
            println!("Too few bytes received: {:?}", n);
            return None;
        },
        Ok(n) => n,
        Err(e) => {
            println!("Error reading request: {:?}", e);
            return None;
        }
    };

    if (n-2) as u8 != buf[1] {
        println!("Methods and size mismatch: {} vs {}", n-2, buf[1]);
        return None;
    }

    return Some(Connect{
        ver: buf[0],
        methods: buf[2..][0..buf[1] as usize].to_vec(),
    });
}

#[derive(Debug)]
enum Command {
    Connect = 1,
    Bind = 2,
    Udp = 3,
}

impl Command {
    fn from(i: u8) -> std::io::Result<Self> {
        match i {
            1 => Ok(Command::Connect),
            2 => Ok(Command::Bind),
            3 => Ok(Command::Udp),
            _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("Invalid command {:02x}", i))),
        }
    }
}

#[derive(Debug)]
enum Addr {
    Ip(std::net::IpAddr),
    Domain(String)
}

impl Addr {
    fn from(mut cur: &mut std::io::Cursor<&[u8]>) -> std::io::Result<Self> {
        return match ReadBytesExt::read_u8(&mut cur)? {
            1 => {
                let mut buf = [0; 4];
                cur.read_exact(&mut buf)?;
                Ok(Addr::Ip(IpAddr::from(buf)))
            },
            4 => {
                let mut buf = [0; 16];
                cur.read_exact(&mut buf)?;
                Ok(Addr::Ip(IpAddr::from(buf)))
            },
            3 => {
                let len = ReadBytesExt::read_u8(&mut cur)?;
                let mut buf = vec![0u8; len as usize];
                cur.read_exact(&mut buf)?;
                match String::from_utf8(buf) {
                   Ok(name) => Ok(Addr::Domain(name)),
                   Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, format!("{:?}", e)))
                }
            },
            e => Err(std::io::Error::new(std::io::ErrorKind::Other, format!("Invalid address type {:02x}", e)))
        };
    }
}

#[derive(Debug)]
struct Request {
    cmd: Command,
    addr: Addr,
    port: u16
}

async fn readRequest(socket: &mut tokio::net::TcpStream) -> std::io::Result<Request> {
    let mut buf = [0; 260];
    let n = match tokio::io::AsyncReadExt::read(socket, &mut buf).await {
        Ok(n) if n < 7 => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Too few bytes received: {:?}", n)));
        },
        Ok(n) => n,
        Err(e) => return Err(e)
    };

    let buf = &buf[0..n];
    let mut cursor = Cursor::new(buf);

    println!("{:?}", &buf);

    let ver = ReadBytesExt::read_u8(&mut cursor)?;
    if 0x05 != ver {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Expected version 0x05, got {:02x}", ver)));
    }

    let cmd = Command::from(ReadBytesExt::read_u8(&mut cursor)?)?;
    cursor.seek(std::io::SeekFrom::Current(1))?;
    let addr = Addr::from(&mut cursor)?;
    let port = ReadBytesExt::read_u16::<NetworkEndian>(&mut cursor)?;

    return Ok(Request{cmd, addr, port});
}

async fn handleConnection(mut socket: &mut TcpStream) -> tokio::io::Result<()> {
    let connect = match readConnect(&mut socket).await {
        None => return Err(tokio::io::Error::new(tokio::io::ErrorKind::Other, "Connection read failed")),
        Some(connect) => connect
    };
    println!("VER={:02x} METHODS={:?}", connect.ver, connect.methods);

    // FIXME check for 0x05 and method==0
    //

    if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut socket, &[0x05u8,0x00]).await {
        return Err(tokio::io::Error::new(tokio::io::ErrorKind::Other, format!("socket write error: {:?}", e)))
    }

    let request = match readRequest(&mut socket).await {
        Err(e) =>
            return Err(tokio::io::Error::new(tokio::io::ErrorKind::Other, format!("Request read failed: {:?}", e))),
        Ok(request) => request
    };
    println!("Request: {:?}", request);

    /* FIXME wtf
    let remote_addr = match request.addr {
        Addr::Ip(ip) => { return ; }, //(&ip.to_string(), request.port),
        Addr::Domain(d) => (&d, request.port)
    };
    */

    let mut outgoing = match request.addr {
        Addr::Ip(_) =>
            return Err(tokio::io::Error::new(tokio::io::ErrorKind::Other, "Ip not implemented FIXME wtf rust")),
        Addr::Domain(d) => {
            let remote_addr = format!("{}:{}", d, request.port);
            match TcpStream::connect(&remote_addr).await {
                Err(e) =>
                    return Err(tokio::io::Error::new(tokio::io::ErrorKind::Other, format!("Connect to {:?} failed: {:?}", remote_addr, e))),
                Ok(socket) => socket
            }
        }
    };

    println!("Connected");

    if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut socket, &[0x05u8,0x00,0x00,0x01,0,0,0,0,0,0]).await {
        return Err(tokio::io::Error::new(tokio::io::ErrorKind::Other, format!("socket write error: {:?}", e)))
    }


    let (mut ri, mut wi) = socket.split();
    let (mut ro, mut wo) = outgoing.split();

    let client_to_server = tokio::io::copy(&mut ri, &mut wo);
    let server_to_client = tokio::io::copy(&mut ro, &mut wi);

    // TODO on error
    let _ = try_join!(client_to_server, server_to_client);

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut listener = TcpListener::bind("127.0.0.1:10000").await?;

    loop {
        let (mut socket, _) = listener.accept().await?;

        tokio::spawn(async move {
            match handleConnection(&mut socket).await {
                Err(e) => println!("Connection error: {:?}", e),
                Ok(_) => {}
            }
        });
    }
}
