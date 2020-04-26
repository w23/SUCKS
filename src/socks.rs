use {
    std::{
        // cell::{RefCell},
        io::{Write, Read, Cursor, Seek},
        net::{IpAddr, ToSocketAddrs},
        // rc::Rc,
    },
    mio::{
        net::{
            TcpStream, TcpListener,
        },
    },
    log::{info, trace, warn, error, debug},
};
use byteorder::{NetworkEndian, ReadBytesExt};

use crate::ringbuf;
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
// enum ConnectionState {
//     Handshake,
//     Request,
//     Connect,
//     Transfer
// }
//
// struct Connection {
//     client_socket: Option<ste::StreamSocket>,
//     state: ConnectionState,
// }
//
// impl Connection {
//     fn new() -> Connection {
//         Connection {
//             client_socket: None,
//             state: ConnectionState::Handshake,
//         }
//     }
//
//     fn client_handler(this: Rc<RefCell<Connection>>) -> ste::StreamHandler {
//         let clone = this.clone();
//         let created = Box::new(move |socket| {
//             clone.borrow_mut().created(socket)
//         });
//         let clone = this.clone();
//         let push = Box::new(move |buf: &[u8]| {
//             clone.borrow_mut().push(buf)
//         });
//         ste::StreamHandler {
//             created,
//             push,
//             pull: Box::new(move |buf| {
//                 this.borrow_mut().pull(buf)
//             }),
//             error: Box::new(move |err| {
//                 error!("Connection error: {:?}", err);
//             }),
//         }
//     }
//
//     fn created(&mut self, socket: ste::StreamSocket) {
//         self.client_socket = Some(socket);
//     }
//
//     fn push(&mut self, buf: &[u8]) -> usize {
//         debug!("push {}", buf.len());
//
//         match self.state {
//             ConnectionState::Handshake => {
//                 // TODO: handle buffer wraparound
//                 let connect = match readConnect(buf) {
//                     None => return buf.len(),
//                     Some(connect) => {
//                         connect
//                     },
//                 };
//
//                 trace!("Received connect: {:?}", connect);
//                 // FIXME write(..).unwrap? seriously?
//                 trace!("Written: {}", self.client_socket.as_mut().unwrap().write(&[0x05u8,0x00]).unwrap());
//
//                 self.state = ConnectionState::Request;
//                 return buf.len();
//             },
//             ConnectionState::Request => {
//                 // TODO: handle buffer wraparound
//                 let request = match readRequest(buf) {
//                     Ok(request) => {
//                         request
//                     },
//                     Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
//                         // FIXME: code dedup
//                         return buf.len();
//                     },
//                     Err(err) => {
//                         error!("Error reading request: {:?}", err);
//                         return buf.len();
//                     },
//                 };
//
//                 info!("Read request: {:?}", request);
//
//                 // FIXME create socket to remote machine
//
//                 // FIXME write(..).unwrap? seriously?
//                 trace!("Written: {}", self.client_socket.as_mut().unwrap().write(&[0x05u8,0x00,0x00,0x01,0,0,0,0,0,0]).unwrap());
//
//                 self.state = ConnectionState::Connect;
//                 return buf.len();
//             },
//             ConnectionState::Connect => {
//                 unimplemented!("");
//             },
//             ConnectionState::Transfer => {
//                 unimplemented!("");
//             }
//         }
//     }
//
//     fn pull(&mut self, buf: &mut [u8])-> usize {
//         debug!("pull {}", buf.len());
//         0
//     }
// }

enum ConnectionState {
    Handshake,
    Request,
    Transfer,
    TransferTail,
}

struct Connection {
    handle: Option<ste::Handle>,
    state: ConnectionState,
    client: TcpStream,
    remote: Option<TcpStream>,
    client_read_buf: ringbuf::RingByteBuffer,
    client_write_buf: ringbuf::RingByteBuffer,
}

impl Connection {
    fn new(client: TcpStream) -> Connection {
        Connection {
            handle: None,
            state: ConnectionState::Handshake,
            client,
            remote: None,
            client_read_buf: ringbuf::RingByteBuffer::new(),
            client_write_buf: ringbuf::RingByteBuffer::new(),
        }
    }
}

impl ste::Context for Connection {
    fn registered(&mut self, ste: &mut ste::Ste, handle: ste::Handle) {
        info!("Handle: {:?}", handle);
        self.handle = Some(handle);
        ste.register_source(handle, &mut self.client, 0);
    }

    fn event(&mut self, ste: &mut ste::Ste, token: usize, event: &mio::event::Event) {
        match token {
            0 => self.handleClient(ste, event),
            1 => self.handleServer(event),
            _ => error!("Unexpected socket token {}", token)
        }
    }
}

impl Connection {
    fn handleServer(&mut self, event: &mio::event::Event) {
        unimplemented!();
    }

    fn handleClient(&mut self, ste: &mut ste::Ste, event: &mio::event::Event) {
        if event.is_readable() {
            loop {
                let (read, drained) = {
                    let buf = self.client_read_buf.get_free_slot();
                    if buf.len() == 0 { break; }
                    let read = match self.client.read(buf) {
                        Ok(size) => size,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                        Err(e) => {
                            error!("Cannot read from client socket: {:?}", e);
                            // FIXME what to do now?!
                            return;
                        }
                    };
                    (read, read < buf.len())
                };
                self.client_read_buf.produce(read);
                if drained { break; }
            }
        }

        if event.is_writable() {
            loop {
                let (written, full) = {
                    let buf = self.client_write_buf.get_data();
                    if buf.len() == 0 { break; }
                    let written = match self.client.write(buf) {
                        Ok(size) => size,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                        Err(e) => {
                            error!("Cannot write to client socket: {:?}", e);
                            // FIXME what to do now?!
                            return;
                        }
                    };
                    (written, written < buf.len())
                };
                self.client_read_buf.consume(written);
                if full { break; }
            }
        }

        match self.state {
            ConnectionState::Handshake => {
                let buf = self.client_read_buf.get_data();
                let connect = match readConnect(buf) {
                    Some(connect) => {
                        // FIXME consume properly
                        let to_drop = buf.len();
                        self.client_read_buf.consume(to_drop);
                        connect
                    },
                    None => return,
                };

                trace!("Received connect: {:?}", connect);
                // FIXME write(..).unwrap? seriously?
                trace!("Written: {}", self.client.write(&[0x05u8,0x00]).unwrap());

                self.state = ConnectionState::Request;
            },
            ConnectionState::Request => {
                // TODO: handle buffer wraparound
                let buf = self.client_read_buf.get_data();
                let request = match readRequest(buf) {
                    Ok(request) => {
                        // FIXME consume properly
                        let to_drop = buf.len();
                        self.client_read_buf.consume(to_drop);
                        request
                    },
                    Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                        return;
                    },
                    Err(err) => {
                        error!("Error reading request: {:?}, {:?}", err, buf);
                        unimplemented!();
                        //return;
                    },
                };

                info!("Read request: {:?}", request);

                // FIXME create socket to remote machine
                //ste.connect_stream(/*&request.addr*/ "localhost", self.handle.unwrap());
                self.remote = Some(match TcpStream::connect(request.socket_addr()) {
                    Ok(socket) => socket,
                    Err(e) => {
                        error!("Cannot connect to remote {:?}: {:?}", request, e);
                        unimplemented!();
                    }
                });

                match ste.register_source(self.handle.unwrap(), self.remote.as_mut().unwrap(), 1) {
                    Err(e) => {
                        error!("Cannot register remote socket {:?}: {:?}", request, e);
                        unimplemented!();
                    }
                    _ => {},
                }

                // FIXME write(..).unwrap? seriously?
                trace!("Written: {}", self.client.write(&[0x05u8,0x00,0x00,0x01,0,0,0,0,0,0]).unwrap());

                self.state = ConnectionState::Transfer;
            },
            ConnectionState::Transfer => {
                unimplemented!("");
            }
        }

    }
}

struct ListenContext {
    socket: mio::net::TcpListener,
}

impl ListenContext {
    fn listen(bind_addr: &str) -> Result<ListenContext, std::io::Error> {
        let listen_addr = bind_addr.to_socket_addrs()?.next().unwrap();
        Ok(ListenContext{
            socket: mio::net::TcpListener::bind(listen_addr)?
        })
    }
}

impl ste::Context for ListenContext {
    fn registered(&mut self, ste: &mut ste::Ste, handle: ste::Handle) {
        ste.register_source(handle, &mut self.socket, 0).unwrap();
    }

    fn event(&mut self, ste: &mut ste::Ste, token: usize, event: &mio::event::Event) {
        debug!("token:{}, event:{:?}", token, event);

        // FIXME if error

        if !event.is_readable() {
            error!("what");
            return;
        }

        loop {
            let accepted = match self.socket.accept() {
                Ok((socket, _)) => socket,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                Err(e) => {
                    error!("cannot accept: {:?}", e);
                    // FIXME handle?
                    return;
                }
            };

            match ste.register_context(Box::new(Connection::new(accepted))) {
                Err(e) => {
                    error!("Cannot register new connection: {:?}", e);
                    // FIXME handle?
                },
                _ => {}
            }
        }
    }
}

pub fn main(listen: &str, exit: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut ste = ste::Ste::new(128).unwrap();

    let context_handle = ste.register_context(Box::new(ListenContext::listen(listen)?))?;
    //let context = ste.get_context(context_handle)?;

    //let listen = ste.listen(listen, context_handle);
    //info!("Listening socket: {:?}", listen);

    ste.run()
}
