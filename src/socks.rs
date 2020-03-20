use {
    std::{
        io::{Write, Read, Cursor, Seek},
        net::{IpAddr, ToSocketAddrs},
    },
    mio::{
        Events, Poll, PollOpt, Ready, Token,
        net::{
            //UdpSocket,
            TcpStream, TcpListener,
        },
    },
    ochenslab::OchenSlab,
    log::{info, trace, warn, error, debug},
};
use byteorder::{NetworkEndian, ReadBytesExt};

use crate::ste;

#[derive(Debug)]
struct Connect {
    ver: u8,
    methods: Vec<u8>
}

fn readConnect(buf: &[u8]) -> Option<Connect> {
    if buf.len() < 3 { return None; }
    if (buf.len()-2) as u8 != buf[1] { return None; }

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
    port: u16,
}

impl Request {
    // FIXME return:
    // - result
    // - vec[]
    fn socket_addr(&self) -> std::net::SocketAddr {
        match &self.addr {
            Addr::Ip(ip) =>
                std::net::SocketAddr::new(*ip, self.port),
            Addr::Domain(d) => {
                let mut addrs_iter = format!("{}:{}", d, self.port).to_socket_addrs().unwrap();
                addrs_iter.next().unwrap()
            }
        }
    }
}

fn readRequest(buf: &[u8]) -> std::io::Result<Request> {
    if buf.len() < 7 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!("Too few bytes received: {:?}", buf.len())));
    }

    let mut cursor = Cursor::new(buf);

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

const TEST_BUFFER_SIZE: usize = 8192;

struct RingByteBuffer {
    buffer: [u8; TEST_BUFFER_SIZE],
    write: usize,
    read: usize,
}

impl RingByteBuffer {
    fn new() -> RingByteBuffer {
        RingByteBuffer {
            buffer: [0; TEST_BUFFER_SIZE],
            write: 0,
            read: 0,
        }
    }

    fn queue_size(&self) -> usize {
        if self.write >= self.read {
            self.write - self.read
        } else {
            self.buffer.len() - (self.read - self.write) - 1
        }
    }

    fn is_empty(&self) -> bool {
        self.write == self.read
    }

    fn is_full(&self) -> bool {
        (self.write + 1) % self.buffer.len() == self.read
    }

    fn get_free_slot(&mut self) -> &mut [u8] {
        if self.write >= self.read {
            &mut self.buffer[self.write..]
        } else {
            &mut self.buffer[self.write..self.read - 1]
        }
    }

    fn produce(&mut self, written: usize) {
        // FIXME check written validity
        self.write = (self.write + written) % self.buffer.len();
    }

    fn get_data(&self) -> &[u8] {
        if self.write >= self.read {
            &self.buffer[self.read..self.write]
        } else {
            &self.buffer[self.read..]
        }
    }

    fn consume(&mut self, read: usize) {
        // FIXME check read validity
        self.read = (self.read + read) % self.buffer.len();
    }
}

impl std::io::Write for RingByteBuffer {
    fn write(&mut self, src: &[u8]) -> std::io::Result<usize> {
        let mut written = 0;
        let mut src = src;
        while !src.is_empty() {
            let dst = self.get_free_slot();
            let write = if dst.len() < src.len() {
                dst.copy_from_slice(&src[..dst.len()]);
                dst.len()
            } else {
                dst[..src.len()].copy_from_slice(src);
                src.len()
            };

            self.produce(write);
            written += write;
            src = &src[write..];
        }

        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// impl Connection {
//     fn handleClientRequest(&mut self, ctx: &ConnectionContext) -> Result<(), std::io::Error> {
//         let request = readRequest(&mut self.client_stream)?;
//         info!("C{}: readRequest => {:?}", self.seq, request);
//
//         match request.cmd {
//             Command::Connect => {},
//             _ => return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("Command {:?} not implemented", request.cmd)))
//         }
//
//         let remote_addr = request.socket_addr();
//         self.test_remote_stream = Some(TcpStream::connect(&remote_addr)?);
//         self.test_handle_remote_stream = Some(Connection::handleRemoteData);
//         ctx.register(self.seq, 1, self.test_remote_stream.as_ref().unwrap())?;
//
//         // Reply to client
//         // FIXME WouldBlock
//         if let Err(e) = self.client_stream.write(&[0x05u8,0x00,0x00,0x01,0,0,0,0,0,0]) {
//            return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("socket write error: {:?}", e)))
//         }
//
//         self.handle_client_stream = Connection::handleClientData;
//         Ok(())
//     }
//
//     fn handleClientData(&mut self, _ctx: &ConnectionContext) -> Result<(), std::io::Error> {
//         // FIXME push data in only available direction
//         transerfignRealnosti(self.test_remote_stream.as_mut().unwrap(), &mut self.client_stream, &mut self.test_from_remote)?;
//         transerfignRealnosti(&mut self.client_stream, self.test_remote_stream.as_mut().unwrap(), &mut self.test_to_remote)
//     }
//
//     fn handleRemoteData(&mut self, _ctx: &ConnectionContext) -> Result<(), std::io::Error> {
//         transerfignRealnosti(&mut self.client_stream, self.test_remote_stream.as_mut().unwrap(), &mut self.test_to_remote)?;
//         transerfignRealnosti(self.test_remote_stream.as_mut().unwrap(), &mut self.client_stream, &mut self.test_from_remote)
//     }
//
//     fn deregister(&self, poll: &Poll) -> Result<(), std::io::Error> {
//         trace!("C{}: deregister", self.seq);
//         poll.deregister(&self.client_stream)?;
//         if let Some(ref stream) = self.test_remote_stream {
//             poll.deregister(stream)?;
//         }
//
//         Ok(())
//     }
// }
//
// const LISTENER: Token = Token(65535);

enum Connection1State {
    Handshake,
    Request,
    Connect,
    Transfer
}

struct Connection1 {
    from_client: RingByteBuffer,
    to_client: RingByteBuffer,
    client_socket: Option<ste::StreamSocket>,

    state: Connection1State,
}

impl Connection1 {
    fn new() -> Connection1 {
        Connection1 {
            from_client: RingByteBuffer::new(),
            to_client: RingByteBuffer::new(),
            client_socket: None,
            state: Connection1State::Handshake,
        }
    }
}

impl ste::StreamHandler for Connection1 {
    fn created(&mut self, socket: ste::StreamSocket) {
        self.client_socket = Some(socket);
    }

    fn push(&mut self, received: usize) -> Option<&mut [u8]> {
        debug!("push {}", received);
        self.from_client.produce(received);

        if received > 0 {
            match self.state {
                Connection1State::Handshake => {
                    // TODO: handle buffer wraparound
                    let connect = match readConnect(self.from_client.get_data()) {
                        None => return None,
                        Some(connect) => {
                            self.from_client.consume(self.from_client.get_data().len());
                            connect
                        },
                    };

                    trace!("Received connect: {:?}", connect);
                    trace!("Written: {}", self.to_client.write(&[0x05u8,0x00]).unwrap());
                    // TODO: do we need to initiate write?

                    self.state = Connection1State::Request;
                },
                Connection1State::Request => {
                    // TODO: handle buffer wraparound
                    let request = match readRequest(self.from_client.get_data()) {
                        Ok(request) => {
                            self.from_client.consume(self.from_client.get_data().len());
                            request
                        },
                        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                            // FIXME: code dedup
                            return Some(self.from_client.get_free_slot())
                        },
                        Err(err) => {
                            error!("Error reading request: {:?}", err);
                            return None;
                        },
                    };

                    info!("Read request: {:?}", request);

                    // FIXME create socket to remote machine

                    trace!("Written: {}", self.to_client.write(&[0x05u8,0x00,0x00,0x01,0,0,0,0,0,0]).unwrap());
                    // TODO: do we need to initiate send?

                    self.state = Connection1State::Connect;
                },
                Connection1State::Connect => {
                    unimplemented!("");
                },
                Connection1State::Transfer => {
                    unimplemented!("");
                }
            }
        }

        Some(self.from_client.get_free_slot())
    }

    fn pull(&mut self, sent: usize) -> Option<&[u8]> {
        debug!("pull {}", sent);
        self.to_client.consume(sent);
        Some(self.to_client.get_data())
    }
}

pub fn main(listen: &str, exit: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut ste = ste::Ste::new(128).unwrap();
    let listener = ste::SocketListener::new(listen, Box::new(|| {
        info!("lol");
        Ok(Box::new(Connection1::new()))
    }))?;
    ste.listen(listener)?;
    ste.run()
}
