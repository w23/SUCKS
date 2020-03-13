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

struct Connect {
    ver: u8,
    methods: Vec<u8>
}

fn readConnect(stream: &mut TcpStream) -> Result<Connect, std::io::Error> {
    let mut buf = [0; 258];
    let n = match stream.read(&mut buf) {
        Ok(n) if n < 3 => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Too few bytes received: {:?}", n)));
        },
        Ok(n) => n,
        Err(e) => return Err(e),
    };

    if (n-2) as u8 != buf[1] {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Methods and size mismatch: {} vs {}", n-2, buf[1])));
    }

    return Ok(Connect{
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

fn readRequest(stream: &mut TcpStream) -> std::io::Result<Request> {
    let mut buf = [0; 260];
    let n = match stream.read(&mut buf) {
        Ok(n) if n < 7 => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Too few bytes received: {:?}", n)));
        },
        Ok(n) => n,
        Err(e) => return Err(e) // FIXME WouldBlock
    };

    let buf = &buf[0..n];
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

const MAX_CONNECTIONS: usize = 128;

struct ConnectionContext<'a> {
    index: usize,
    token: usize,
    poll: &'a Poll,
}

impl<'a> ConnectionContext<'a> {
    fn new(index: usize, token: usize, poll: &'a Poll) -> ConnectionContext<'a> {
        ConnectionContext { index, token, poll }
    }

    fn register<'b>(&'a self, seq: u32, token: usize, stream: &'b TcpStream) -> Result<(), std::io::Error> {
        self.poll.register(stream,
            Token(self.index + token * MAX_CONNECTIONS + (seq << 16) as usize),
            Ready::readable() | Ready::writable(), PollOpt::edge())
    }
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

fn transerfignRealnosti(src: &mut TcpStream, dst: &mut TcpStream, buf: &mut RingByteBuffer) -> Result<(), std::io::Error> {
    let mut write_blocked = false;
    let mut read_blocked = false;
    loop {
        // First, try to write everything we have
        while !write_blocked { // TODO can we write everything in one call?
            let data = buf.get_data();
            if data.len() == 0 {
                if read_blocked { return Ok(()); }
                break;
            }
            match dst.write(data) {
                Ok(written) => {
                    buf.consume(written);
                    trace!("written={}", written);
                },
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    write_blocked = true;
                    if read_blocked || buf.is_full() {
                        return Ok(());
                    }
                },
                Err(e) => {
                    if buf.queue_size() == 0 {
                        unimplemented!("FIXME: handle {:?}", e);
                    }
                }
            }
        }

        // Then try to read
        trace!("buffer size = {}", buf.queue_size());
        while !read_blocked { // TODO can we read everything in one call?
            let buffer = buf.get_free_slot();
            let buffer_size = buffer.len();
            if buffer.len() == 0 {
                if write_blocked { return Ok(()); }
                break;
            }
            match src.read(buffer) {
                Ok(read) => {
                    if read == 0 {
                        // Means that socket should be closed
                        // FIXME handle sending remaining data
                        return Err(std::io::Error::new(std::io::ErrorKind::Other, "Socket closed"));
                    }

                    buf.produce(read);
                    trace!("read={}", read);

                    // Do not make second attempt of reading if first one haven't written the
                    // entire buffer
                    if read != buffer_size {
                        read_blocked = true;
                        if write_blocked || buf.is_empty() {
                            return Ok(());
                        }
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    read_blocked = true;
                    if write_blocked || buf.is_empty() {
                        return Ok(());
                    }
                },
                Err(e) => {
                    if buf.queue_size() == 0 {
                        unimplemented!("FIXME: handle {:?}", e);
                    }
                },
            };
        }
    }
}

type ConnectionHandleFn = fn(&mut Connection, &ConnectionContext) -> Result<(), std::io::Error>;

struct Connection {
    seq: u32,
    client_stream: TcpStream,
    //exit_socket: UdpSocket,

    test_remote_stream: Option<TcpStream>,
    test_to_remote: RingByteBuffer,
    test_from_remote: RingByteBuffer,

    handle_client_stream: ConnectionHandleFn,
    test_handle_remote_stream: Option<ConnectionHandleFn>,
}

impl Connection {
    fn new(seq: u32, stream: TcpStream) -> Connection {
        Connection {
            seq,
            client_stream: stream,
            test_remote_stream: None,
            handle_client_stream: Connection::handleClientNewConnection,
            test_handle_remote_stream: None,
            test_to_remote: RingByteBuffer::new(),
            test_from_remote: RingByteBuffer::new(),
        }
    }

    fn handle(&mut self, ctx: &ConnectionContext, readiness: mio::Ready) -> Result<(), std::io::Error> {

        // FIXME don't do this
        if readiness.is_error() {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "socket error"));
        }

        let handle: Option<ConnectionHandleFn> =
            if ctx.token == 0 {
                Some(self.handle_client_stream)
            } else {
                Some(*self.test_handle_remote_stream.as_ref().unwrap())
            };

        match (handle.unwrap())(self, ctx) {
            Err(e) if e.kind() != std::io::ErrorKind::WouldBlock => {
                // FIXME what to do?
                Err(e)
            },
            _ => Ok(())
        }
    }

    fn handleClientNewConnection(&mut self, _ctx: &ConnectionContext) -> Result<(), std::io::Error> {
        let connect = readConnect(&mut self.client_stream)?;
        info!("C{}: readConnect => VER={:02x} METHODS={:?}", self.seq, connect.ver, connect.methods);
        // FIXME check for 0x05 and method==0

        // FIXME check for wouldblock ?
        if let Err(e) = self.client_stream.write(&[0x05u8,0x00]) {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("socket write error: {:?}", e)))
        }

        self.handle_client_stream = Connection::handleClientRequest;
        Ok(())
    }

    fn handleClientRequest(&mut self, ctx: &ConnectionContext) -> Result<(), std::io::Error> {
        let request = readRequest(&mut self.client_stream)?;
        info!("C{}: readRequest => {:?}", self.seq, request);

        match request.cmd {
            Command::Connect => {},
            _ => return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("Command {:?} not implemented", request.cmd)))
        }

        let remote_addr = request.socket_addr();
        self.test_remote_stream = Some(TcpStream::connect(&remote_addr)?);
        self.test_handle_remote_stream = Some(Connection::handleRemoteData);
        ctx.register(self.seq, 1, self.test_remote_stream.as_ref().unwrap())?;

        // Reply to client
        // FIXME WouldBlock
        if let Err(e) = self.client_stream.write(&[0x05u8,0x00,0x00,0x01,0,0,0,0,0,0]) {
           return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("socket write error: {:?}", e)))
        }

        self.handle_client_stream = Connection::handleClientData;
        Ok(())
    }

    fn handleClientData(&mut self, _ctx: &ConnectionContext) -> Result<(), std::io::Error> {
        // FIXME push data in only available direction
        transerfignRealnosti(self.test_remote_stream.as_mut().unwrap(), &mut self.client_stream, &mut self.test_from_remote)?;
        transerfignRealnosti(&mut self.client_stream, self.test_remote_stream.as_mut().unwrap(), &mut self.test_to_remote)
    }

    fn handleRemoteData(&mut self, _ctx: &ConnectionContext) -> Result<(), std::io::Error> {
        transerfignRealnosti(&mut self.client_stream, self.test_remote_stream.as_mut().unwrap(), &mut self.test_to_remote)?;
        transerfignRealnosti(self.test_remote_stream.as_mut().unwrap(), &mut self.client_stream, &mut self.test_from_remote)
    }

    fn deregister(&self, poll: &Poll) -> Result<(), std::io::Error> {
        trace!("C{}: deregister", self.seq);
        poll.deregister(&self.client_stream)?;
        if let Some(ref stream) = self.test_remote_stream {
            poll.deregister(stream)?;
        }

        Ok(())
    }
}

const LISTENER: Token = Token(65535);

#[derive(Debug)]
struct Connection1 {
}

impl ste::StreamHandler for Connection1 {
    fn push(&mut self) {
        debug!("{:?} push", self);
    }

    fn pull(&mut self) {
        debug!("{:?} pull", self);
    }
}

pub fn main(listen: &str, exit: &str) -> Result<(), Box<dyn std::error::Error>> {
    if exit == "ste"
    {
        let mut ste = ste::Ste::new().unwrap();
        let listener = ste::Listen::new(listen, Box::new(|| {
            info!("lol");
            Ok(Box::new(Connection1{}))
        }))?;
        ste.listen(listener)?;
        ste.run()?;
    }

    let poll = Poll::new()?;
    let mut events = Events::with_capacity(128);

    let listen_addr = listen.to_socket_addrs()?.next().unwrap();
    let listener = TcpListener::bind(&listen_addr)?;
    poll.register(&listener, LISTENER, Ready::readable(), PollOpt::edge())?;

    let mut connections = OchenSlab::<Connection>::with_capacity(MAX_CONNECTIONS);

    let mut conn_seq: u32 = 0;

    loop {
        trace!("loop. connections={}", connections.len());
        poll.poll(&mut events, None).unwrap();

        for event in &events {
            debug!("event: {:?}", event);
            match event.token() {
                LISTENER => {
                    loop {
                        match listener.accept() {
                            Ok((socket, _)) => {
                                conn_seq += 1;
                                let index = connections.insert(Connection::new(conn_seq, socket));
                                if index.is_none() {
                                    error!("Too many connections: {}", connections.len());
                                    break;
                                }
                                let index = index.expect("Too many connections");
                                let token = Token(index + (conn_seq << 16) as usize);
                                let conn = connections.get_mut(index).unwrap();
                                info!("C{}: token {} for socket {:?}", conn.seq, token.0, conn.client_stream);
                                poll.register(&conn.client_stream, token,
                                    Ready::readable() | Ready::writable(), PollOpt::edge())?;
                            },
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                            Err(e) => return Err(Box::new(e))
                        }
                    }
                },
                Token(t) => {
                    // FIXME token version vs connection version
                    let seq = (t >> 16) as u32;
                    let index = (t & 0xffff) % MAX_CONNECTIONS;
                    let token = (t & 0xffff) / MAX_CONNECTIONS;
                    trace!("Token({}) -> seq={}, index={}, token={}", t, seq, index, token);
                    if let Some(ref mut conn) = connections.get_mut(index) {
                        if conn.seq != seq {
                            warn!("Mismatched sequence for C{}: expected {}", seq, conn.seq);
                        } else {
                            //match connections.get_mut(index).unwrap().handle(&ConnectionContext::new(index, token, &poll)) {
                            match conn.handle(&ConnectionContext::new(index, token, &poll), event.readiness()) {
                                Err(e) if e.kind() != std::io::ErrorKind::WouldBlock => {
                                    conn.deregister(&poll)?;
                                    connections.remove(index);
                                },
                                _ => {},
                            };
                        }
                    }
                }
                //token => panic!("what! {:?}", token)
            }
        }
    }
}
