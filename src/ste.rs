use {
    std::{
        //borrow::{BorrowMut},
        cell::{RefCell},
        rc::Rc,
        io::{Write, Read, Cursor, Seek},
        net::{IpAddr, ToSocketAddrs},
        ops::DerefMut,
    },
    mio::{
        Events, Poll, PollOpt, Ready, Token,
        net::{
            //UdpSocket,
            TcpStream, TcpListener,
        },
    },
    log::{info, trace, warn, error, debug},
    ochenslab::OchenSlab,
};

use crate::ringbuf;

pub struct StreamSocket {
    socket: SocketRc
}

impl StreamSocket {
    fn new(socket: SocketRc) -> StreamSocket {
        StreamSocket { socket }
    }

    pub fn close(&mut self) {
        unimplemented!("");
    }
}

impl std::io::Write for StreamSocket {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // FIXME use send buffer
        match self.socket.borrow_mut().deref_mut() {
            SocketKind::Tcp(sock) => sock.stream_socket.write(buf),
            _ => { unimplemented!(""); }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        unimplemented!("");
    }
}

pub struct StreamHandler {
    pub created: Box<dyn FnMut(StreamSocket)>,
    pub push: Box<dyn FnMut(&[u8]) -> usize>,
    pub pull: Box<dyn FnMut(&mut [u8]) -> usize>,
    pub error: Box<dyn FnMut(std::io::Error)>,
}

type ListenCallback = dyn Fn() -> Result<StreamHandler, std::io::Error>;

struct SocketListener
{
    listener: mio::net::TcpListener,
    callback: Box<ListenCallback>,
}

impl SocketListener {
    fn new(bind_addr: &str, callback: Box<ListenCallback>) -> Result<SocketListener, std::io::Error> {
        let listen_addr = bind_addr.to_socket_addrs()?.next().unwrap();
        Ok(SocketListener {
            listener: mio::net::TcpListener::bind(&listen_addr)?,
            callback: callback,
        })
    }
}

struct SocketTcp {
    stream_socket: TcpStream,
    stream_handler: StreamHandler,
    read_buf: ringbuf::RingByteBuffer,
}

impl SocketTcp {
    fn new(stream_socket: TcpStream, stream_handler: StreamHandler) -> SocketTcp {
        SocketTcp { stream_socket, stream_handler, read_buf: ringbuf::RingByteBuffer::new() }
    }

    fn handle(&mut self, ready: mio::Ready) -> Result<(), Box<dyn std::error::Error>> {
        // TODO handle matrix:
        // 1. have data to send
        // 2. have buffer space to receive
        // 3. ready: can send
        // 4. ready: can recv

        if ready.is_error() {
            unimplemented!("readiness error not implemented");
        }
        if ready.is_hup() {
            unimplemented!("readiness hup not implemented");
        }
        if ready.is_readable() {
            loop {
                let mut buffer_full = self.read_buf.is_full();
                let mut socket_drained = false;

                while !socket_drained && !buffer_full {
                    let (buf_len, read) = {
                        let buf = self.read_buf.get_free_slot();
                        (buf.len(), self.stream_socket.read(buf)?)
                    };
                    self.read_buf.produce(read);

                    socket_drained = read < buf_len;
                    buffer_full = self.read_buf.is_full();
                }

                let mut buffer_empty = self.read_buf.is_empty();
                let mut client_full = false;
                while !client_full && !buffer_empty {
                    let (buf_len, consumed) = {
                        let buf = self.read_buf.get_data();
                        (buf.len(), (self.stream_handler.push)(buf))
                    };
                    self.read_buf.consume(consumed);

                    client_full = consumed < buf_len;
                    buffer_empty = self.read_buf.is_empty();
                }

                if (socket_drained && buffer_empty) ||
                    (client_full && buffer_full) {
                        break;
                }
            }
        }
        if ready.is_writable() {
        }
        Ok(())
    }
}

enum SocketKind {
    Listener(SocketListener),
    Tcp(SocketTcp),
}

impl SocketKind {
    fn get_mio_evented(&mut self) -> &mut dyn mio::event::Evented {
       match self {
            SocketKind::Listener(listen) => &mut listen.listener,
            SocketKind::Tcp(stream) => &mut stream.stream_socket,
        }
    }

    fn created(&mut self, socket: StreamSocket) {
        match self {
            SocketKind::Tcp(ref mut stream) => {
                let handler = &mut stream.stream_handler;
                (handler.created)(socket)
            }
            SocketKind::Listener(_) => {},
        }
    }
}

type SocketRc = Rc<RefCell<SocketKind>>;

struct VersionedSocket {
    seq: usize,
    socket: SocketRc
}

impl VersionedSocket {
    fn new(seq: usize, socket: Rc<RefCell<SocketKind>>) -> VersionedSocket {
        VersionedSocket { seq, socket }
    }

    fn get(&self, seq: usize) -> Option<&Rc<RefCell<SocketKind>>> {
        if self.seq != seq {
            return None;
        }

        return Some(&self.socket);
    }
}

type SocketsContainer = OchenSlab::<VersionedSocket>;

pub struct Ste {
    poll: Poll,
    sockets: SocketsContainer,
    seq: usize,
}

impl Ste {
    pub fn new(max_sockets: usize) -> Result<Ste, Box<dyn std::error::Error>> {
        Ok(Ste {
            poll: Poll::new()?,
            sockets: SocketsContainer::with_capacity(max_sockets),
            seq: 0,
        })
    }

    fn get_seq(&mut self) -> usize {
        self.seq += 1;
        self.seq
    }

    // FIXME return closable/droppable object
    pub fn listen(&mut self, bind_addr: &str, callback: Box<ListenCallback>) -> Result<(), std::io::Error> {
        let listener = SocketListener::new(bind_addr, callback)?;
        let seq = self.get_seq();
        let socket = Rc::new(RefCell::new(SocketKind::Listener(listener)));
        let index = match self.sockets.insert(VersionedSocket::new(seq, socket.clone())) {
            None => { return Err(std::io::Error::new(std::io::ErrorKind::Other, "Capacity exceeded")); },
            Some(index) => index
        };

        let token = Token(index + (self.seq << 16));

        let mut socket = socket.borrow_mut();
        let listen = socket.get_mio_evented();
        info!("S{}: token {} for listened socket", seq, token.0);
        self.poll.register(listen, token,
            Ready::readable() | Ready::writable(), PollOpt::edge())
    }

    //pub fn connectTcp(&mut self, sock_addr: std::net::SocketAddr) -> Result<

    fn handleListen(&mut self, ready: mio::Ready, sock: &mut SocketListener) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            match sock.listener.accept() {
                Ok((socket, _)) => {
                    info!("New connect: {:?}", socket);

                    let seq = self.get_seq();
                    let stream = SocketTcp::new(socket, (sock.callback)()?);
                    let socket_rc = Rc::new(RefCell::new(SocketKind::Tcp(stream)));

                    let token = Token(match self.sockets.insert(VersionedSocket::new(seq, socket_rc.clone())) {
                        None => {
                            // FIXME tell user that we've failed
                            //error!("Cannot insert {:?}, no slots available", socket);
                            return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, "sockets slots exhausted")));
                        },
                        Some(index) => index
                    } | (seq << 16));

                    let mut socket = socket_rc.borrow_mut();

                    info!("S{}: token {}", self.seq, token.0);
                    // FIXME if register failed we should let user know that socket failed
                    self.poll.register(socket.get_mio_evented(), token,
                        Ready::readable() | Ready::writable(), PollOpt::edge())?;

                    socket.created(StreamSocket::new(socket_rc.clone()));
                },
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                Err(e) => return Err(Box::new(e))
            }
        }

        Ok(())
    }

    fn handleTcp(&mut self, ready: mio::Ready, sock: &mut SocketTcp) -> Result<(), Box<dyn std::error::Error>> {
        return sock.handle(ready);
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut events = Events::with_capacity(128);

        loop {
            trace!("loop. sockets={}", self.sockets.len());
            self.poll.poll(&mut events, None).unwrap();

            for event in &events {
                let t = event.token().0;
                let seq = t >> 16;
                let index = (t & 0xffff) % self.sockets.capacity();

                debug!("event: {:?}", event);

                let sock = match self.sockets.get(index) {
                    Some(sock) => sock,
                    None => {
                        warn!("S{}: stale event, no such socket", index);
                        return Ok(());
                    }
                };

                let sock = match sock.get(seq) {
                    Some(sock) => sock,
                    None => {
                        warn!("S{} stale seq {} received, slot has {}", index, seq, sock.seq);
                        return Ok(());
                    }
                }.clone();

                let mut sock = sock.borrow_mut();

                let result = match sock.deref_mut() {
                    SocketKind::Listener(listen) => self.handleListen(event.readiness(), listen),
                    SocketKind::Tcp(stream) => self.handleTcp(event.readiness(), stream),
                };

                match result {
                    Ok(_) => {},
                    Err(err) => error!("S{}: error {:?}", index, err),
                };
            }
        }
    }
}
