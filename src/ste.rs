use {
    std::{
        //borrow::{BorrowMut},
        cell::{RefCell},
        rc::Rc,
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
    log::{info, trace, warn, error, debug},
    ochenslab::OchenSlab,
};

pub trait StreamHandler {
    fn push(&mut self);
    fn pull(&mut self);
}

type ListenCallback = dyn Fn() -> Result<Box<dyn StreamHandler>, std::io::Error>;
//type ListenCallback = dyn Fn() -> Result<(), std::io::Error>;

pub struct Listen
{
    listener: mio::net::TcpListener,
    callback: Box<ListenCallback>,
}

impl Listen {
    pub fn new(bind_addr: &str, callback: Box<ListenCallback>) -> Result<Listen, Box<dyn std::error::Error>> {
        let listen_addr = bind_addr.to_socket_addrs()?.next().unwrap();
        Ok(Listen {
            listener: mio::net::TcpListener::bind(&listen_addr)?,
            callback: callback,
        })
    }
}

struct Stream {
    stream_socket: TcpStream,
    stream_handler: Box<dyn StreamHandler>
}

impl Stream {
    fn new(stream_socket: TcpStream, stream_handler: Box<dyn StreamHandler>) -> Stream {
        Stream { stream_socket, stream_handler }
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

enum SocketData {
    Listener(Listen),
    Stream(Stream),
}

struct Socket {
    seq: usize,
    data: SocketData,
}

impl Socket {
    fn from_listen(seq: usize, listen: Listen) -> Socket {
        Socket {
            seq,
            data: SocketData::Listener(listen),
        }
    }
    fn from_stream(seq: usize, stream: Stream) -> Socket {
        Socket {
            seq,
            data: SocketData::Stream(stream),
        }
    }

    fn getMioSocket(&mut self) -> &mut dyn mio::event::Evented {
       match &mut self.data {
            SocketData::Listener(listen) => &mut listen.listener,
            SocketData::Stream(stream) => &mut stream.stream_socket,
        }
    }

    fn dummy(&mut self) {
    }
}

type SocketsContainer = OchenSlab::<Rc<RefCell<Socket>>>;

pub struct Ste {
    poll: Poll,
    sockets: SocketsContainer,
    seq: usize,
}

const MAX_SOCKETS: usize = 128;

impl Ste {
    pub fn new() -> Result<Ste, Box<dyn std::error::Error>> {
        Ok(Ste {
            poll: Poll::new()?,
            sockets: SocketsContainer::with_capacity(MAX_SOCKETS),
            seq: 0,
        })
    }

    fn get_seq(&mut self) -> usize {
        self.seq += 1;
        self.seq
    }

    pub fn listen(&mut self, listener: Listen) -> Result<(), std::io::Error> {
        let seq = self.get_seq();
        let socket = Rc::new(RefCell::new(Socket::from_listen(seq, listener)));
        let index = match self.sockets.insert(Rc::clone(&socket)) {
            None => { return Err(std::io::Error::new(std::io::ErrorKind::Other, "Capacity exceeded")); },
            Some(index) => index
        };

        let token = Token(index + (self.seq << 16));

        let mut socket = socket.borrow_mut();
        let listen = socket.getMioSocket();
        info!("S{}: token {} for listened socket", seq, token.0);
        self.poll.register(listen, token,
            Ready::readable() | Ready::writable(), PollOpt::edge())
    }

    fn handleSocketIndex(&mut self, index: usize, seq: usize, ready: mio::Ready) -> Result<(), Box<dyn std::error::Error>> {
        let sock = match self.sockets.get(index) {
            Some(sock) => sock,
            None => {
                warn!("Stale event for S{}", seq);
                return Ok(());
            }
        }.clone();
        let mut sock = sock.borrow_mut();

        if sock.seq != seq {
            warn!("S{} Stale seq {} received, slot has {}", index, seq, sock.seq);
            return Ok(());
        }

        // TODO how to nest less
        match &mut sock.data {
            SocketData::Listener(listen) => loop {
                match listen.listener.accept() {
                    Ok((socket, _)) => {
                        info!("New connect: {:?}", socket);

                        let seq = self.get_seq();
                        let stream = Stream::new(socket, (listen.callback)()?);
                        let socket = Rc::new(RefCell::new(Socket::from_stream(seq, stream)));

                        let token = Token(match self.sockets.insert(Rc::clone(&socket)) {
                            None => {
                                //error!("Cannot insert {:?}, no slots available", socket);
                                return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, "sockets slots exhausted")));
                            },
                            Some(index) => index
                        } | (seq << 16));

                        info!("S{}: token {}", self.seq, token.0);
                        self.poll.register(socket.borrow_mut().getMioSocket(), token,
                            Ready::readable() | Ready::writable(), PollOpt::edge())?;
                    },
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                    Err(e) => return Err(Box::new(e))
                }
            },
            SocketData::Stream(ref mut stream) => {
                debug!("S{} event {:?}", index, ready);
                return stream.handle(ready);
            },
        }

        Ok(())
    }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut events = Events::with_capacity(128);

        loop {
            trace!("loop. sockets={}", self.sockets.len());
            self.poll.poll(&mut events, None).unwrap();

            for event in &events {
                debug!("event: {:?}", event);
                let t = event.token().0;
                let seq = t >> 16;
                let index = (t & 0xffff) % MAX_SOCKETS;

                self.handleSocketIndex(index, seq, event.readiness())?
            }
        }
    }
}
