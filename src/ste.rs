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

pub trait StreamHandler {
    fn push(&mut self);
    fn pull(&mut self);
}

type ListenCallback = dyn Fn() -> Result<Box<dyn StreamHandler>, std::io::Error>;
//type ListenCallback = dyn Fn() -> Result<(), std::io::Error>;

pub struct SocketListener
{
    listener: mio::net::TcpListener,
    callback: Box<ListenCallback>,
}

impl SocketListener {
    pub fn new(bind_addr: &str, callback: Box<ListenCallback>) -> Result<SocketListener, Box<dyn std::error::Error>> {
        let listen_addr = bind_addr.to_socket_addrs()?.next().unwrap();
        Ok(SocketListener {
            listener: mio::net::TcpListener::bind(&listen_addr)?,
            callback: callback,
        })
    }
}

struct SocketTcp {
    stream_socket: TcpStream,
    stream_handler: Box<dyn StreamHandler>
}

impl SocketTcp {
    fn new(stream_socket: TcpStream, stream_handler: Box<dyn StreamHandler>) -> SocketTcp {
        SocketTcp { stream_socket, stream_handler }
    }

    fn handle(&mut self, ready: mio::Ready) -> Result<(), Box<dyn std::error::Error>> {
        if ready.is_error() {
            unimplemented!("readiness error not implemented");
        }
        if ready.is_hup() {
            unimplemented!("readiness hup not implemented");
        }
        if ready.is_readable() {
            self.stream_handler.push();
        }
        if ready.is_writable() {
            self.stream_handler.pull();
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
}

struct VersionedSocket {
    seq: usize,
    socket: Rc<RefCell<SocketKind>>
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

    pub fn listen(&mut self, listener: SocketListener) -> Result<(), std::io::Error> {
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

    fn handleListen(&mut self, ready: mio::Ready, sock: &mut SocketListener) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            match sock.listener.accept() {
                Ok((socket, _)) => {
                    info!("New connect: {:?}", socket);

                    let seq = self.get_seq();
                    let stream = SocketTcp::new(socket, (sock.callback)()?);
                    let socket = Rc::new(RefCell::new(SocketKind::Tcp(stream)));

                    let token = Token(match self.sockets.insert(VersionedSocket::new(seq, socket.clone())) {
                        None => {
                            //error!("Cannot insert {:?}, no slots available", socket);
                            return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, "sockets slots exhausted")));
                        },
                        Some(index) => index
                    } | (seq << 16));

                    info!("S{}: token {}", self.seq, token.0);
                    self.poll.register(socket.borrow_mut().get_mio_evented(), token,
                        Ready::readable() | Ready::writable(), PollOpt::edge())?;

                    // FIXME if register failed we should let user know that socket failed
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
