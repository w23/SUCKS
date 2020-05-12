use {
    std::{
        io::{Write, Read, Cursor, Seek, IoSlice, IoSliceMut},
        net::{IpAddr, ToSocketAddrs},
        // rc::Rc,
    },
    mio::{
        net::{
            TcpStream, TcpListener,
        },
    },
    circbuf::CircBuf,
    ::log::{info, trace, warn, error, debug},
};
use byteorder::{NetworkEndian, ReadBytesExt};

use crate::ste;
use crate::log;

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

struct ReadWritePipe<T: Read+Write> {
    pipe: T,
    readable: bool,
    writable: bool,
}

impl<T: Read+Write> ReadWritePipe<T> {
    fn new(pipe: T) -> ReadWritePipe<T> {
        ReadWritePipe{pipe, readable: false, writable: false }
    }

    fn read(&mut self, buffer: &mut CircBuf) -> std::io::Result<usize> {
        if buffer.cap() == 0 { return Ok(0) }
        let [buf1, buf2] = buffer.get_avail();
        let mut bufs = [IoSliceMut::new(buf1), IoSliceMut::new(buf2)];
        match self.pipe.read_vectored(&mut bufs) {
            Ok(0) => {
                self.readable = false;
                Ok(0)
            },
            Ok(read) => {
                buffer.advance_write(read);
                Ok(read)
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                self.readable = false;
                Ok(0)
            },
            Err(e) => Err(e),
        }
    }

    fn write(&mut self, buffer: &mut CircBuf) -> std::io::Result<usize> {
        if buffer.len() == 0 { return Ok(0) }
        let bufs = buffer.get_bytes();
        let bufs = [IoSlice::new(&bufs[0]), IoSlice::new(&bufs[1])];
        match self.pipe.write_vectored(&bufs) {
            Ok(0) => {
                self.writable = false;
                Ok(0)
            },
            Ok(written) => {
                buffer.advance_read(written);
                Ok(written)
            },
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                self.writable = false;
                Ok(0)
            },
            Err(e) => Err(e),
        }
    }
}

struct Tcp {
    pipe: ReadWritePipe<TcpStream>,
    handle: ste::Handle,
}

impl Tcp {
    fn from_stream(ste: &mut ste::Ste, context: ste::Handle, mut stream: TcpStream, token: usize) -> std::io::Result<Tcp> {
        let handle = ste.register_source(context, &mut stream, token)?;
        Ok(Tcp {
            pipe: ReadWritePipe::new(stream),
            handle
        })
    }

    fn connect(ste: &mut ste::Ste, context: ste::Handle, addr: std::net::SocketAddr, token: usize) -> std::io::Result<Tcp> {
        let mut stream = TcpStream::connect(addr)?;
        let handle = ste.register_source(context, &mut stream, token)?;
        Ok(Tcp { pipe: ReadWritePipe::new(stream), handle })
    }

    fn deregister(&mut self, ste: &mut ste::Ste) {
        match ste.deregister_source(self.handle, &mut self.pipe.pipe) {
            Err(e) => { error!("Error deregistering socket: {}", e) },
            _ => {},
        }
    }

    fn update_state(&mut self, event: &mio::event::Event) -> std::io::Result<()> {
        trace!("readable {} -> {}, writable {} -> {}", self.pipe.readable, event.is_readable(), self.pipe.writable, event.is_writable());
        if event.is_readable() { self.pipe.readable = true; }
        if event.is_writable() { self.pipe.writable = true; }

        if event.is_read_closed() != event.is_write_closed() {
            error!("read_closed {} != write_closed {}", event.is_read_closed(), event.is_write_closed());
        }

        if event.is_read_closed() || event.is_write_closed() {
            debug!("Remote socket closed");
            self.pipe.readable = false;
            self.pipe.writable = false;
            return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, ""));
        }

        if event.is_error() {
            error!("Remote socket error");
            self.pipe.readable = false;
            self.pipe.writable = false;
            return Err(std::io::Error::new(std::io::ErrorKind::Other, ""));
        }

        Ok(())
    }
}

#[derive(PartialEq)]
enum ConnectionState {
    Handshake,
    Request,
    Transfer,
}

struct Connection {
    handle: Option<ste::Handle>,
    lol_socket: Option<TcpStream>,
    state: ConnectionState,
    client: Option<Tcp>,
    remote: Option<Tcp>,
    buf_from_client: CircBuf,
    buf_to_client: CircBuf,
}

impl ste::Context for Connection {
    fn registered(&mut self, ste: &mut ste::Ste, handle: ste::Handle) {
        info!("Handle: {:?}", handle);
        self.handle = Some(handle);
        let socket = self.lol_socket.take().unwrap();
        self.client = Some(Tcp::from_stream(ste, handle, socket, 0).unwrap()) // FIXME don't unwrap please
    }

    fn event(&mut self, ste: &mut ste::Ste, token: usize, event: &mio::event::Event) {
        log_scope!(format!("C{}: ", self.handle.unwrap()));
        match token {
            0 => {
                log_scope!("client: ");
                match self.handleClient(ste, event) {
                    Err(e) => {
                        error!("Client socket error: {}", e);
                        self.client.take().unwrap().deregister(ste);
                    },
                    Ok(cont) if !cont => return,
                    _ => {},
                }
            },
            1 => {
                log_scope!("remote: ");
                let remote = self.remote.as_mut().unwrap();
                match remote.update_state(event) {
                    Err(e) => {
                        error!("Remote socket error: {}", e);
                        remote.deregister(ste);
                        self.remote = None
                    },
                    _ => {},
                }
            }
            _ => error!("Unexpected socket token {}", token)
        }

        loop {
            {
                log_scope!("client -> remote: ");
                feed(ste, &mut self.client, &mut self.remote, &mut self.buf_from_client);
            }
            {
                log_scope!("remote -> client: ");
                feed(ste, &mut self.remote, &mut self.client, &mut self.buf_to_client);
            }

            if (self.client.is_none() && self.buf_from_client.is_empty()) ||
                (self.remote.is_none() && self.buf_to_client.is_empty()) ||
                (self.client.is_none() && self.remote.is_none())
            {
                self.disconnect(ste);
                return
            }

            let client_readable = self.client.is_some() && self.client.as_mut().unwrap().pipe.readable;
            let remote_readable = self.remote.is_some() && self.remote.as_mut().unwrap().pipe.readable;
            let client_writable = self.client.is_some() && self.client.as_mut().unwrap().pipe.writable;
            let remote_writable = self.remote.is_some() && self.remote.as_mut().unwrap().pipe.writable;

            if ((!client_readable && self.buf_from_client.is_empty()) || !remote_writable) &&
                ((!remote_readable && self.buf_to_client.is_empty()) || !client_writable) {
                    break
            }
        }
    }
}

fn feed(ste: &mut ste::Ste, source: &mut Option<Tcp>, dest: &mut Option<Tcp>, buf: &mut CircBuf) {
    let received = match source {
        Some(tcp) => {
            match tcp.pipe.read(buf) {
                Err(e) => {
                    error!("Error reading socket: {}", e);
                    tcp.deregister(ste);
                    *source = None;
                    0
                }, Ok(read) => read,
            }
        }, _ => 0,
    };
    debug!("received: {} to buffer: {}", received, buf.len());
    if buf.is_empty() { return }

    let sent = match dest {
        Some(tcp) => {
            match tcp.pipe.write(buf) {
                Err(e) => {
                    error!("Error writing socket: {}", e);
                    tcp.deregister(ste);
                    *dest = None;
                    0
                }, Ok(written) => written,
            }
        }, _ => 0
    };

    debug!("sent: {}, left in buffer: {}", sent, buf.len());
}


const TEST_BUFFER_SIZE: usize = 8192;

impl Connection {
    fn new(client: TcpStream) -> Connection {
        Connection {
            handle: None,
            lol_socket: Some(client),
            state: ConnectionState::Handshake,
            client: None,
            remote: None,
            buf_from_client: CircBuf::with_capacity(TEST_BUFFER_SIZE).unwrap(),
            buf_to_client: CircBuf::with_capacity(TEST_BUFFER_SIZE).unwrap(),
        }
    }

    fn disconnect_remote(&mut self, ste: &mut ste::Ste) {
        if self.remote.is_none() { return }
        self.remote.take().unwrap().deregister(ste);
    }

    fn disconnect_client(&mut self, ste: &mut ste::Ste) {
        if self.client.is_none() { return }
        self.client.take().unwrap().deregister(ste);
    }

    fn disconnect(&mut self, ste: &mut ste::Ste) {
        self.disconnect_client(ste);
        self.disconnect_remote(ste);
        if self.handle.is_some() {
            match ste.deregister_context(self.handle.unwrap()) {
                Err(e) => warn!("Couldn't deregister client socket: {}", e),
                _ => {},
            }
        }
    }

    fn handleClient(&mut self, ste: &mut ste::Ste, event: &mio::event::Event) -> std::io::Result<bool> {
        let client = self.client.as_mut().unwrap();
        client.update_state(event)?;
        client.pipe.read(&mut self.buf_from_client)?;
        match self.state {
            ConnectionState::Transfer => {
                return Ok(true)
            }
            ConnectionState::Handshake => {
                let buf = self.buf_from_client.get_bytes()[0];
                let connect = match readConnect(buf) {
                    Some(connect) => {
                        // FIXME consume properly
                        let to_drop = buf.len();
                        self.buf_from_client.advance_read(to_drop);
                        connect
                    },
                    None => return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid SOCKS handshake")),
                };

                trace!("Received connect: {:?}", connect);
                self.buf_to_client.write(&[0x05u8,0x00]).unwrap();
                self.state = ConnectionState::Request;
            },
            ConnectionState::Request => {
                // TODO: handle buffer wraparound
                let buf = self.buf_from_client.get_bytes()[0];
                let request = readRequest(buf)?;
                // FIXME consume properly
                let to_drop = buf.len();
                self.buf_from_client.advance_read(to_drop);
                info!("Read request: {:?}", request);

                // TODO DNS
                self.remote = Some(Tcp::connect(ste, self.handle.unwrap(), request.socket_addr(), 1)?);
                self.buf_to_client.write(&[0x05u8,0x00,0x00,0x01,0,0,0,0,0,0]).unwrap();
                self.state = ConnectionState::Transfer;
            },
        }
        client.pipe.write(&mut self.buf_to_client)?;
        Ok(false)
    }
}

struct ListenContext {
    socket: TcpListener,
}

impl ListenContext {
    fn listen(bind_addr: &str) -> Result<ListenContext, std::io::Error> {
        // TODO DNS
        let listen_addr = bind_addr.to_socket_addrs()?.next().unwrap();
        Ok(ListenContext{
            socket: TcpListener::bind(listen_addr)?
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

    let _context_handle = ste.register_context(Box::new(ListenContext::listen(listen)?))?;
    ste.run()
}
