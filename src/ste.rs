use {
    std::{
        //borrow::{BorrowMut},
        // io::{Write, Read, Cursor, Seek},
        net::{/*IpAddr,*/ ToSocketAddrs},
        // ops::DerefMut,
    },
    mio::{
        Events, Poll, Token,
        // net::{
        //     UdpSocket,
        //     TcpStream, TcpListener,
        // },
    },
    log::{info, trace, warn, error, debug},
    ochenslab::OchenSlab,
};

enum MetaSocketKind {
    Listener(mio::net::TcpListener),
    Tcp(mio::net::TcpStream),
}

struct MetaSocket {
    context: ContextHandle,
    socket: MetaSocketKind,
}

const INTERESTS: mio::Interest = mio::Interest::READABLE.add(mio::Interest::WRITABLE);

impl MetaSocket {
    fn create_listen(context: ContextHandle, bind_addr: &str) -> Result<MetaSocket, std::io::Error> {
        let listen_addr = bind_addr.to_socket_addrs()?.next().unwrap();
        Ok(MetaSocket{ context, socket: MetaSocketKind::Listener(mio::net::TcpListener::bind(listen_addr)?)})
    }
    fn register(&mut self, registry: &mio::Registry, token: Token) -> std::io::Result<()> {
        match &mut self.socket {
            MetaSocketKind::Listener(listen) => registry.register(listen, token, INTERESTS),
            MetaSocketKind::Tcp(stream) => registry.register(stream, token, INTERESTS),
        }
    }
}

struct Versioned<T> {
    seq: usize,
    value: T,
}

impl<T> Versioned<T> {
    fn new(seq: usize, value: T) -> Versioned<T> {
        Versioned { seq, value }
    }

    fn get(&self, seq: usize) -> Option<&T> {
        if self.seq != seq {
            return None;
        }

        return Some(&self.value);
    }

    fn get_mut(&mut self, seq: usize) -> Option<&mut T> {
        if self.seq != seq {
            return None;
        }

        return Some(&mut self.value);
    }
}

pub trait Context {
    // pub created: Box<dyn FnMut(StreamSocket)>,
    // pub push: Box<dyn FnMut(&[u8]) -> usize>,
    // pub pull: Box<dyn FnMut(&mut [u8]) -> usize>,
    // pub error: Box<dyn FnMut(std::io::Error)>,
}

pub struct ContextHandle {
    index: usize,
    seq: usize,
}

impl ContextHandle {
    fn new(index: usize, seq: usize) -> ContextHandle {
        ContextHandle { index, seq }
    }
}

pub struct SocketHandle {
    index: usize,
    seq: usize,
}

impl SocketHandle {
    fn new(index: usize, seq: usize) -> SocketHandle {
        SocketHandle { index, seq }
    }

    pub fn close() {
        unimplemented!();
    }
}

pub struct Sequence {
    seq: usize,
}

impl Sequence {
    fn new() -> Sequence {
        Sequence { seq: 0 }
    }

    fn next(&mut self) -> usize {
        let ret = self.seq;
        self.seq += 1;
        ret
    }
}

type SocketsContainer = OchenSlab::<Versioned<MetaSocket>>;
type ContextsContainer = OchenSlab::<Versioned<Box<dyn Context>>>;

pub struct Ste {
    poll: Poll,
    seq: Sequence,
    sockets: SocketsContainer,
    contexts: ContextsContainer,
}

impl Ste {
    pub fn new(max_sockets: usize) -> Result<Ste, Box<dyn std::error::Error>> {
        Ok(Ste {
            poll: Poll::new()?,
            seq: Sequence::new(),
            sockets: SocketsContainer::with_capacity(max_sockets),
            contexts: ContextsContainer::with_capacity(max_sockets), // FIXME max_contexts
        })
    }

    pub fn register_context(&mut self, context: Box<dyn Context>) -> Result<ContextHandle, std::io::Error> {
        let seq = self.seq.next();
        let index = match self.contexts.insert(Versioned::<Box<dyn Context>>::new(seq, context)) {
            None => { return Err(std::io::Error::new(std::io::ErrorKind::Other, "Capacity exceeded")); },
            Some(index) => index
        };

        info!("C{}: [{}]", seq, index);
        Ok(ContextHandle::new(index, seq))
    }

    pub fn connect_stream(&mut self, address: &str, context: ContextHandle) -> Result<SocketHandle, std::io::Error> {
        unimplemented!();
    }

    pub fn listen(&mut self, address: &str, context: ContextHandle) -> Result<SocketHandle, std::io::Error> {
        let socket = MetaSocket::create_listen(context, address)?;
        let seq = self.seq.next();
        let index = match self.sockets.insert(Versioned::<MetaSocket>::new(seq, socket)) {
            None => { return Err(std::io::Error::new(std::io::ErrorKind::Other, "Capacity exceeded")); },
            Some(index) => index
        };

        let token = Token(index + (seq << 16));
        let socket = self.sockets.get_mut(index).unwrap().get_mut(seq).unwrap();
        socket.register(&self.poll.registry(), token);

        info!("S{}: token {} for listened socket", seq, token.0);
        Ok(SocketHandle::new(index, seq))
    }

    // fn handleListen(&mut self, ready: mio::Ready, sock: &mut SocketListener) -> Result<(), Box<dyn std::error::Error>> {
    //     loop {
    //         match sock.listener.accept() {
    //             Ok((socket, _)) => {
    //                 info!("New connect: {:?}", socket);
    //
    //                 let seq = self.get_seq();
    //                 let stream = SocketTcp::new(socket, (sock.callback)()?);
    //                 let socket_rc = Rc::new(RefCell::new(MetaSocket::Tcp(stream)));
    //
    //                 let token = Token(match self.sockets.insert(VersionedSocket::new(seq, socket_rc.clone())) {
    //                     None => {
    //                         // FIXME tell user that we've failed
    //                         //error!("Cannot insert {:?}, no slots available", socket);
    //                         return Err(Box::new(std::io::Error::new(std::io::ErrorKind::Other, "sockets slots exhausted")));
    //                     },
    //                     Some(index) => index
    //                 } | (seq << 16));
    //
    //                 let mut socket = socket_rc.borrow_mut();
    //
    //                 info!("S{}: token {}", self.seq, token.0);
    //                 // FIXME if register failed we should let user know that socket failed
    //                 self.poll.register(socket.get_mio_evented(), token,
    //                     Ready::readable() | Ready::writable(), PollOpt::edge())?;
    //
    //                 socket.created(StreamSocket::new(socket_rc.clone()));
    //             },
    //             Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
    //             Err(e) => return Err(Box::new(e))
    //         }
    //     }
    //
    //     Ok(())
    // }

    pub fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let mut events = Events::with_capacity(128);

        loop {
            trace!("loop. sockets={}", self.sockets.len());
            self.poll.poll(&mut events, None).unwrap();

            'events: for event in &events {
                let t = event.token().0;
                let seq = t >> 16;
                let index = (t & 0xffff) % self.sockets.capacity();

                debug!("event: {:?}", event);

                let sock = match self.sockets.get_mut(index) {
                    Some(sock) => sock,
                    None => {
                        warn!("S{}: stale event, no such socket", index);
                        continue;
                    }
                };

                let sock: &mut MetaSocket = match sock.get_mut(seq) {
                    Some(sock) => sock,
                    None => {
                        warn!("S{} stale seq {} received, slot has {}", index, seq, sock.seq);
                        continue;
                    }
                };

                let context = match self.contexts.get_mut(sock.context.index) {
                    Some(context) => context,
                    None => {
                        error!("C{} is stale, but associated socket still exists", sock.context.index);
                        unimplemented!("FIXME this socket should die now");
                        //continue;
                    }
                };

                let context: &mut Box<dyn Context> = match context.get_mut(sock.context.seq) {
                    Some(context) => context,
                    None => {
                        error!("C{} is stale, expected seq is {} but got {} and associated socket still exists", sock.context.index, sock.context.seq, context.seq);
                        unimplemented!("FIXME this socket should die now");
                        //continue;
                    }
                };

                match &sock.socket {
                    MetaSocketKind::Listener(sock) => {
                        loop {
                            let accepted = match sock.accept() {
                                Ok((socket, _)) => socket,
                                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => { break; },
                                Err(e) => {
                                    //return Err(Box::new(e))
                                    error!("S{}, cannot accept: {:?}", index, e);
                                    // FIXME notify context
                                    continue 'events;
                                }
                            };
                        }
                    },
                    MetaSocketKind::Tcp(sock) => {
                    }
                }


                // let result = match sock.deref_mut() {
                //     MetaSocket::Listener(listen) => self.handleListen(event.readiness(), listen),
                //     MetaSocket::Tcp(stream) => self.handleTcp(event.readiness(), stream),
                // };
                //
                // match result {
                //     Ok(_) => {},
                //     Err(err) => error!("S{}: error {:?}", index, err),
                // };
            }
        }
    }
}
