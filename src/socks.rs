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

use crate::ringbuf::RingByteBuffer;
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

struct ReadWritePipe<T: Read+Write> {
    pipe: T,
    drained: bool,
    full: bool,
}

impl<T: Read+Write> ReadWritePipe<T> {
    fn new(pipe: T) -> ReadWritePipe<T> {
        ReadWritePipe{pipe, drained: true, full: true }
    }

    fn read(&mut self, buffer: &mut RingByteBuffer) -> std::io::Result<usize> {
        let mut total = 0;
        loop {
            if self.drained { break; }
            let read = {
                let buf = buffer.get_free_slot();
                if buf.len() == 0 { break; }
                let read = match self.pipe.read(buf) {
                    Ok(size) => size,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                    Err(e) => return Err(e),
                };
                self.drained = read < buf.len();
                read
            };
            total += read;
            buffer.produce(read);
        }
        Ok(total)
    }

    fn write(&mut self, buffer: &mut RingByteBuffer) -> std::io::Result<usize> {
        let mut total = 0;
        loop {
            if self.full { break; }
            let written = {
                let buf = buffer.get_data();
                if buf.len() == 0 { break; }
                let written = match self.pipe.write(buf) {
                    Ok(size) => size,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                    Err(e) => return Err(e),
                };
                self.full = written < buf.len();
                written
            };
            total += written;
            buffer.consume(written);
        }
        Ok(total)
    }

    fn feed<U: Read+Write>(&mut self, from: &mut ReadWritePipe<U>, buffer: &mut RingByteBuffer) -> (std::io::Result<usize>, std::io::Result<usize>) {
        // FIXME compute sum of all sent
        loop {
            let ret = (from.read(buffer), self.write(buffer));
            if ret.0.is_err() || ret.1.is_err() || *ret.0.as_ref().ok().unwrap() == 0 || *ret.1.as_ref().ok().unwrap() == 0 {
                return ret;
            }
        }
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
    state: ConnectionState,
    client: Option<ReadWritePipe<TcpStream>>,
    client_handle: Option<ste::Handle>,
    remote: Option<ReadWritePipe<TcpStream>>,
    remote_handle: Option<ste::Handle>,
    buf_from_client: RingByteBuffer,
    buf_to_client: RingByteBuffer,
}

impl ste::Context for Connection {
    fn registered(&mut self, ste: &mut ste::Ste, handle: ste::Handle) {
        info!("Handle: {:?}", handle);
        self.handle = Some(handle);
        self.client_handle = Some(ste.register_source(handle, &mut self.client.as_mut().unwrap().pipe, 0).unwrap()); // FIXME don't unwrap please
    }

    fn event(&mut self, ste: &mut ste::Ste, token: usize, event: &mio::event::Event) {
        match token {
            0 => {
                match self.handleClient(ste, event) {
                    Err(e) => {
                        error!("Client socket error: {}", e);
                        self.closeClient(ste);
                    },
                    _ => {},
                }
            },
            1 => {
                match self.handleRemote(ste, event) {
                    Err(e) => {
                        error!("Client socket error: {}", e);
                        self.closeRemote(ste);
                    },
                    _ => {},
                }
            }
            _ => error!("Unexpected socket token {}", token)
        }
    }
}

impl Connection {
    fn new(client: TcpStream) -> Connection {
        Connection {
            handle: None,
            state: ConnectionState::Handshake,
            client: Some(ReadWritePipe::new(client)),
            client_handle: None,
            remote: None,
            remote_handle: None,
            buf_from_client: RingByteBuffer::new(),
            buf_to_client: RingByteBuffer::new(),
        }
    }

    fn disconnect_remote(&mut self, ste: &mut ste::Ste) {
        if self.remote.is_none() { return }
        match ste.deregister_source(self.remote_handle.unwrap(), &mut self.remote.as_mut().unwrap().pipe) {
            Err(e) => warn!("Couldn't deregister remote socket: {}", e),
            _ => {},
        }
        self.remote_handle = None;
        self.remote = None;

        if self.client_handle.is_none() {
            self.disconnect(ste);
        }
    }

    fn disconnect_client(&mut self, ste: &mut ste::Ste) {
        if self.client.is_none() { return }
        match ste.deregister_source(self.client_handle.unwrap(), &mut self.client.as_mut().unwrap().pipe) {
            Err(e) => warn!("Couldn't deregister client socket: {}", e),
            _ => {},
        }
        self.client_handle = None;
        self.client = None;
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

    fn transfer(&mut self, ste: &mut ste::Ste) {
        match (self.client.as_mut(), self.remote.as_mut()) {
            (Some(client), Some(remote)) => {
                // FIXME cannot reasonably handle errors here, need to warp transfer to outside
                remote.feed(client, &mut self.buf_from_client);
                client.feed(remote, &mut self.buf_to_client);
            },
            (None, Some(remote)) => {
                match remote.write(&mut self.buf_from_client) {
                    Err(e) => {
                        error!("Remote socket error: {}", e);
                        self.closeRemote(ste);
                    },
                    _ => {},
                }
            },
            (Some(client), None) => {
                match client.write(&mut self.buf_to_client) {
                    Err(e) => {
                        error!("Client socket error: {}", e);
                        self.closeClient(ste);
                    },
                    _ => {},
                }
            },
            (_, _) => {},
        }
    }

    fn handleRemote(&mut self, ste: &mut ste::Ste, event: &mio::event::Event) -> std::io::Result<()> {
        assert!(self.state == ConnectionState::Transfer);
        let remote = self.remote.as_mut().unwrap();
        if event.is_readable() { remote.drained = false; }
        if event.is_writable() { remote.full = false; }

        if event.is_read_closed() != event.is_write_closed() {
            error!("read_closed {} != write_closed {}", event.is_read_closed(), event.is_write_closed());
        }

        if event.is_read_closed() || event.is_write_closed() {
            debug!("Remote socket closed");
            return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, ""));
        }

        if event.is_error() {
            error!("Remote socket error");
            return Err(std::io::Error::new(std::io::ErrorKind::Other, ""));
        }

        self.transfer(ste);
        Ok(())
    }

    fn closeRemote(&mut self, ste: &mut ste::Ste) {
        if self.client.is_none() || self.buf_to_client.is_empty() {
            self.disconnect(ste);
        } else {
            self.disconnect_remote(ste);
        }
    }

    fn closeClient(&mut self, ste: &mut ste::Ste) {
        if self.remote.is_none() || self.buf_from_client.is_empty() {
            self.disconnect(ste);
        } else {
            self.disconnect_client(ste);
        }
    }

    fn handleClient(&mut self, ste: &mut ste::Ste, event: &mio::event::Event) -> std::io::Result<()> {
        let client = self.client.as_mut().unwrap();
        if event.is_readable() { client.drained = false; }
        if event.is_writable() { client.full = false; }

        if event.is_read_closed() != event.is_write_closed() {
            error!("read_closed {} != write_closed {}", event.is_read_closed(), event.is_write_closed());
        }

        if event.is_read_closed() || event.is_write_closed() {
            debug!("Client socket closed");
            return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, ""));
        }

        if event.is_error() {
            error!("Client socket error");
            return Err(std::io::Error::new(std::io::ErrorKind::Other, ""));
        }

        client.read(&mut self.buf_from_client)?;

        match self.state {
            ConnectionState::Handshake => {
                let buf = self.buf_from_client.get_data();
                let connect = match readConnect(buf) {
                    Some(connect) => {
                        // FIXME consume properly
                        let to_drop = buf.len();
                        self.buf_from_client.consume(to_drop);
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
                let buf = self.buf_from_client.get_data();
                let request = readRequest(buf)?;
                // FIXME consume properly
                let to_drop = buf.len();
                self.buf_from_client.consume(to_drop);

                info!("Read request: {:?}", request);

                self.remote = Some(match TcpStream::connect(request.socket_addr()) {
                    Ok(socket) => ReadWritePipe::new(socket),
                    Err(e) => {
                        error!("Cannot connect to remote {:?}: {:?}", request, e);
                        unimplemented!();
                    }
                });

                self.remote_handle = match ste.register_source(self.handle.unwrap(), &mut self.remote.as_mut().unwrap().pipe, 1) {
                    Err(e) => {
                        error!("Cannot register remote socket {:?}: {:?}", request, e);
                        unimplemented!();
                    },
                    Ok(handle) => Some(handle),
                };

                self.buf_to_client.write(&[0x05u8,0x00,0x00,0x01,0,0,0,0,0,0]).unwrap();
                self.state = ConnectionState::Transfer;
            },
            ConnectionState::Transfer => {
                self.transfer(ste);
                return Ok(()) // to avoid write below
            }
        }

        match client.write(&mut self.buf_to_client) {
            Err(e) => Err(e),
            Ok(_) => Ok(()),
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

    let _context_handle = ste.register_context(Box::new(ListenContext::listen(listen)?))?;
    //let context = ste.get_context(context_handle)?;

    //let listen = ste.listen(listen, context_handle);
    //info!("Listening socket: {:?}", listen);

    ste.run()
}
