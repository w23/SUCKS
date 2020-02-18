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
    slab::Slab,
};
use byteorder::{NetworkEndian, ReadBytesExt};

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

const MAX_CONNECTIONS: usize = 32;

struct ConnectionContext<'a> {
    index: usize,
    token: usize,
    poll: &'a Poll,
}

impl<'a> ConnectionContext<'a> {
    fn new(index: usize, token: usize, poll: &'a Poll) -> ConnectionContext<'a> {
        ConnectionContext { index, token, poll }
    }

    fn register<'b>(&'a self, token: usize, stream: &'b TcpStream) -> Result<(), std::io::Error> {
        self.poll.register(stream,
            Token(self.index + token * MAX_CONNECTIONS + 1),
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

type ConnectionHandleFn = fn(&mut Connection, &ConnectionContext) -> Result<(), std::io::Error>;

struct Connection {
    client_stream: TcpStream,
    //exit_socket: UdpSocket,

    test_remote_stream: Option<TcpStream>,
    test_to_remote: RingByteBuffer,
    test_from_remote: RingByteBuffer,

    handle_client_stream: ConnectionHandleFn,
    test_handle_remote_stream: Option<ConnectionHandleFn>,
}

impl Connection {
    fn new(stream: TcpStream) -> Connection {
        Connection {
            client_stream: stream,
            test_remote_stream: None,
            handle_client_stream: Connection::handleClientNewConnection,
            test_handle_remote_stream: None,
            test_to_remote: RingByteBuffer::new(),
            test_from_remote: RingByteBuffer::new(),
        }
    }

    fn handle(&mut self, ctx: &ConnectionContext) -> Result<(), std::io::Error> {
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
        println!("VER={:02x} METHODS={:?}", connect.ver, connect.methods);
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
        println!("Request: {:?}", request);

        match request.cmd {
            Command::Connect => {},
            _ => return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("Command {:?} not implemented", request.cmd)))
        }

        let remote_addr = request.socket_addr();
        self.test_remote_stream = Some(TcpStream::connect(&remote_addr)?);
        self.test_handle_remote_stream = Some(Connection::handleRemoteData);
        ctx.register(1, self.test_remote_stream.as_ref().unwrap())?;

        // Reply to client
        // FIXME WouldBlock
        if let Err(e) = self.client_stream.write(&[0x05u8,0x00,0x00,0x01,0,0,0,0,0,0]) {
           return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("socket write error: {:?}", e)))
        }

        self.handle_client_stream = Connection::handleClientData;
        Ok(())
    }

    fn handleClientData(&mut self, _ctx: &ConnectionContext) -> Result<(), std::io::Error> {
        let read = self.client_stream.read(self.test_to_remote.get_free_slot())?;
        self.test_to_remote.produce(read);
        println!("cli->remote read={}", read);

        let data = self.test_to_remote.get_data();
        println!("data={}", data.len());
        let written = self.test_remote_stream.as_ref().unwrap().write(data)?;
        self.test_to_remote.consume(written);
        println!("cli->remote wr={}", written);

        Ok(())

        //Err(std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "Not implemented"))
    }

    fn handleRemoteData(&mut self, _ctx: &ConnectionContext) -> Result<(), std::io::Error> {
        { // FIXME DONT DO THIS
            let data = self.test_to_remote.get_data();
            println!("data={}", data.len());
            match self.test_remote_stream.as_ref().unwrap().write(data) {
                Ok(written) => {
                    self.test_to_remote.consume(written);
                    println!("cli->remote wr={}", written);
                },
                _ => {},
            };
        }

        let read = self.test_remote_stream.as_mut().unwrap().read(self.test_from_remote.get_free_slot())?;
        self.test_from_remote.produce(read);
        println!("cli->remote read={}", read);

        let written = self.client_stream.write(self.test_from_remote.get_data())?;
        self.test_from_remote.consume(written);
        println!("cli->remote wr={}", written);

        Ok(())
    }
}

const LISTENER: Token = Token(0);

pub fn main(listen: &str, exit: &str) -> Result<(), Box<dyn std::error::Error>> {
    let poll = Poll::new()?;
    let mut events = Events::with_capacity(128);

    let listen_addr = listen.parse()?;
    let listener = TcpListener::bind(&listen_addr)?;
    poll.register(&listener, LISTENER, Ready::readable(), PollOpt::edge())?;

    let mut connections = Slab::<Connection>::with_capacity(MAX_CONNECTIONS);

    loop {
        println!("loop");
        poll.poll(&mut events, None).unwrap();

        for event in &events {
            println!("event: {:?}", event);
            match event.token() {
                LISTENER => {
                    match listener.accept() {
                        Ok((socket, _)) => {
                            println!("New socket: {:?}", socket);
                            let token = Token(connections.insert(Connection::new(socket)) + 1);
                            println!("Token {}", token.0);
                            poll.register(&connections.get_mut(token.0-1).unwrap().client_stream, token,
                                Ready::readable() | Ready::writable(), PollOpt::edge())?;
                        },
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                        Err(e) => return Err(Box::new(e))
                    }
                },
                Token(t) => {
                    let index = (t - 1) % MAX_CONNECTIONS;
                    let token = (t - 1) / MAX_CONNECTIONS;
                    println!("Token({}) -> index={}, token={}", t, index, token);
                    connections.get_mut(index).unwrap().handle(&ConnectionContext::new(index, token, &poll))?;
                }
                //token => panic!("what! {:?}", token)
            }
        }
    }
}
